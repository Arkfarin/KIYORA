import { useLayoutEffect, useRef, useState } from "react";
import type { Route } from "../app/navigation";
import { useBack } from "../app/back";

export function useNavigation(session?: string) {
  const [route, setRoute] = useState<Route>({ kind: "home" });
  const stack = useRef<Route[]>([]);
  const positions = useRef(new Map<string, number>());
  const savePosition = () =>
    positions.current.set(
      JSON.stringify(route),
      document.getElementById("main-content")?.scrollTop ?? 0,
    );
  const navigate = (next: Route) => {
    if (JSON.stringify(next) === JSON.stringify(route)) return;
    savePosition();
    stack.current.push(route);
    if (stack.current.length > 100) stack.current.shift();
    window.dispatchEvent(new Event("kiyora:navigate"));
    setRoute(next);
  };
  const back = () => {
    const previous = stack.current.pop();
    if (!previous) return false;
    savePosition();
    setRoute(previous);
    return true;
  };
  useBack(back);
  useLayoutEffect(() => {
    document.getElementById("main-content")?.scrollTo({
      top: positions.current.get(JSON.stringify(route)) ?? 0,
      behavior: "instant",
    });
  }, [route]);
  useLayoutEffect(() => {
    stack.current = [];
    positions.current.clear();
    setRoute({ kind: "home" });
  }, [session]);
  return { route, navigate, back, canGoBack: stack.current.length > 0 };
}
