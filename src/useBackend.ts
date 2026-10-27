import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { bootstrap, call, errorOf, sessionInfo } from "./backend";
import type { ApiError, Auth, BackendEvent, Download, ImportInfo, Job, Playback, Profile, Progress, Projection, Queue, SessionInfo, Status } from "./backend";

export interface Model {
  loading: boolean; profiles: Profile[]; status: Status | null; info: SessionInfo | null;
  auth: Auth | null; projection: Projection | null; playback: Playback; queue: Queue | null;
  imports: ImportInfo[]; jobs: Job[]; downloads: Record<string, Download>; progress: Progress | null;
  error: ApiError | null; revision: number;
}
const empty: Model = { loading: true, profiles: [], status: null, info: null, auth: null, projection: null, playback: { state: "idle" }, queue: null, imports: [], jobs: [], downloads: {}, progress: null, error: null, revision: 0 };
const upsert = <T,>(items: T[], value: T, key: (item: T) => string) => [...items.filter(item => key(item) !== key(value)), value];

export function useBackend() {
  const [model, setModel] = useState<Model>(empty);
  const [busy, setBusy] = useState<string | null>(null);
  const current = useRef(model);
  const refreshRef = useRef<() => Promise<void>>(async () => {});
  const alive = useRef(false);
  const actionId = useRef(0);
  useEffect(() => { current.current = model; }, [model]);

  useEffect(() => {
    let disposed = false;
    alive.current = true;
    let first = true;
    let hydrating = false;
    let again = false;
    let queued: BackendEvent[] = [];
    let lastSequence = 0n;
    let projectionSequence = 0n;
    let warningSequence = 0n;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unlisten: (() => void)[] = [];
    const update = (change: (value: Model) => Model) => {
      if (disposed) return;
      const next = change(current.current);
      current.current = next;
      setModel(next);
    };
    const schedule = () => {
      if (disposed || timer !== undefined) return;
      timer = setTimeout(() => { timer = undefined; void refresh(); }, 180);
    };
    const apply = (event: BackendEvent) => {
      if (disposed) return;
      const sequence = BigInt(event.sequence);
      if (sequence <= lastSequence) return;
      if (lastSequence !== 0n && sequence > lastSequence + 1n) schedule();
      lastSequence = sequence;
      const context = current.current.status?.context;
      if (!context || event.context.session_id !== context.session_id || event.context.profile_id !== context.profile_id) {
        // A different context is only an invalidation. Never adopt its payload;
        // the backend's current status is the authority after a profile switch.
        if (event.name === "runtime.mode.changed") schedule();
        return;
      }
      if (event.context.vault_id !== context.vault_id) { schedule(); return; }
      switch (event.name) {
        case "runtime.mode.changed": update(m => ({ ...m, status: event.payload as Status })); schedule(); break;
        case "auth.state.changed": update(m => ({ ...m, auth: event.payload as Auth })); schedule(); break;
        case "vault.operation.progress": update(m => ({ ...m, progress: event.payload as Progress })); break;
        case "playback.state": update(m => ({ ...m, playback: event.payload as Playback })); break;
        case "playback.queue.changed": update(m => ({ ...m, queue: event.payload as Queue })); break;
        case "vault.warning": warningSequence = sequence; update(m => ({ ...m, error: event.payload as ApiError })); schedule(); break;
        case "import.progress": update(m => ({ ...m, imports: upsert(m.imports, event.payload as ImportInfo, item => item.op_id) })); break;
        case "job.progress": update(m => ({ ...m, jobs: upsert(m.jobs, event.payload as Job, item => item.job_id) })); schedule(); break;
        case "download.progress": { const d = event.payload as Download; update(m => ({ ...m, downloads: { ...m.downloads, [d.track_id]: d } })); break; }
        case "library.changed":
        case "playlist.changed":
          if (sequence > projectionSequence) schedule();
          break;
        case "command.result": schedule(); break;
      }
    };
    const refresh = async () => {
      if (disposed) return;
      if (hydrating) { again = true; return; }
      hydrating = true;
      try {
        const profiles = await call("list_profiles", undefined);
        const status = first ? await bootstrap() : await call("runtime_status", undefined).catch(error => {
          if (errorOf(error).code === "NO_PROFILE") return null;
          throw error;
        });
        first = false;
        if (disposed) return;
        if (!status) {
          update(m => ({ ...empty, profiles, loading: false, error: m.error }));
          return;
        }
        const session_id = status.context.session_id;
        const changed = current.current.status?.context.session_id !== session_id;
        if (changed) {
          projectionSequence = 0n;
          update(() => ({ ...empty, profiles, status, loading: false }));
        }
        // The session watermark precedes every subsequent resource read. Events
        // already represented by those reads must not regress the restored UI.
        const infoSnapshot = await sessionInfo(session_id);
        const results = await Promise.allSettled([
          Promise.resolve(infoSnapshot), call("auth_status", { session_id }), call("playback_state", { session_id }),
          call("playback_queue", { session_id, offset: 0, limit: 1 }), call("list_jobs", { session_id }),
          call("state_snapshot", { session_id }), call("query", { session_id, query: { kind: "imports", payload: { offset: 0, limit: 500 } } }),
        ] as const);
        if (disposed) return;
        // A switch can finish during these independent local reads.
        const latest = await call("runtime_status", undefined);
        if (latest.context.session_id !== session_id || latest.context.vault_id !== status.context.vault_id) { again = true; return; }
        const [info, auth, playback, queue, jobs, projection, imports] = results;
        const baseline = BigInt(infoSnapshot.event_sequence);
        if (baseline > lastSequence) lastSequence = baseline;
        if (projection.status === "fulfilled") projectionSequence = BigInt(projection.value.event_sequence);
        const diagnostics = info.status === "fulfilled" ? info.value.diagnostics : [];
        const warning = [...diagnostics].reverse().find(item => item.error && BigInt(item.sequence) > warningSequence);
        if (warning) warningSequence = BigInt(warning.sequence);
        const progress = [...diagnostics].reverse().find(item => item.progress)?.progress;
        const failure = results.find(result => result.status === "rejected" && !["NO_VAULT", "AUTH_REQUIRED"].includes(errorOf(result.reason).code));
        update(m => ({
          ...m, loading: false, profiles, status: latest,
          info: info.status === "fulfilled" ? info.value : m.info,
          auth: auth.status === "fulfilled" ? auth.value : m.auth,
          playback: playback.status === "fulfilled" ? playback.value : m.playback,
          queue: queue.status === "fulfilled" ? queue.value.info : m.queue,
          jobs: jobs.status === "fulfilled" ? jobs.value : m.jobs,
          projection: projection.status === "fulfilled" ? projection.value : m.projection,
          imports: imports.status === "fulfilled" ? imports.value.result as ImportInfo[] : m.imports,
          error: failure?.status === "rejected" ? errorOf(failure.reason) : warning?.error ?? m.error,
          progress: progress ?? m.progress,
          revision: m.revision + 1,
        }));
      } catch (error) {
        update(m => ({ ...m, loading: false, error: errorOf(error) }));
      } finally {
        hydrating = false;
        const events = queued; queued = [];
        // Snapshot watermarks cover projection invalidations only. Auth and
        // playback have independent snapshots and still need their latest events.
        for (const event of events) apply(event);
        if (again) { again = false; schedule(); }
      }
    };
    refreshRef.current = refresh;
    const attach = async () => {
      try {
        for (const name of ["kiyora://backend", "kiyora://resync-required"]) {
          const off = name === "kiyora://backend"
            ? await listen<BackendEvent>(name, ({ payload }) => {
                if (disposed) return;
                if (hydrating) {
                  if (queued.length < 2048) queued.push(payload);
                  else { queued = []; again = true; }
                } else apply(payload);
              })
            : await listen(name, () => { if (hydrating) again = true; else schedule(); });
          if (disposed) { off(); return; }
          unlisten.push(off);
        }
        await refresh();
      } catch (error) { update(m => ({ ...m, loading: false, error: errorOf(error) })); }
    };
    void attach();
    const fallback = setInterval(schedule, 60_000);
    window.addEventListener("focus", schedule);
    return () => {
      disposed = true; alive.current = false;
      clearInterval(fallback); if (timer !== undefined) clearTimeout(timer);
      window.removeEventListener("focus", schedule);
      for (const off of unlisten) off();
      // Only frontend subscriptions end here. Backend, jobs and audio stay alive.
    };
  }, []);

  const run = useCallback(async <T,>(label: string, operation: () => Promise<T>, refreshAfter = true): Promise<T | undefined> => {
    const ticket = ++actionId.current;
    const session = current.current.status?.context.session_id;
    setBusy(label);
    current.current = { ...current.current, error: null };
    setModel(current.current);
    try { return await operation(); }
    catch (error) {
      if (alive.current && current.current.status?.context.session_id === session) {
        current.current = { ...current.current, error: errorOf(error) };
        setModel(current.current);
      }
      return undefined;
    } finally {
      if (alive.current) {
        if (actionId.current === ticket) setBusy(null);
        if (refreshAfter) await refreshRef.current();
      }
    }
  }, []);
  const refresh = useCallback(() => refreshRef.current(), []);
  const clearError = useCallback(() => {
    current.current = { ...current.current, error: null };
    setModel(current.current);
  }, []);
  return { model, busy, run, refresh, clearError };
}
export type Runtime = ReturnType<typeof useBackend>;
