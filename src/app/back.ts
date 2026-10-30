import { useEffect, useRef } from "react";

// Dialogs register after the underlying screen. Only the top layer consumes Back.
const handlers: (() => boolean)[] = [];
export function goBack() {
  for (let i = handlers.length - 1; i >= 0; i--) {
    if (handlers[i]()) return true;
  }
  return false;
}
window.addEventListener("kiyora:back", (event) => {
  if (goBack()) event.preventDefault();
});
window.addEventListener("keydown", (event) => {
  if (event.altKey && event.key === "ArrowLeft" && goBack())
    event.preventDefault();
});
export function useBack(handler: () => boolean, enabled = true) {
  const latest = useRef(handler);
  latest.current = handler;
  useEffect(() => {
    if (!enabled) return;
    const handle = () => latest.current();
    handlers.push(handle);
    return () => {
      const index = handlers.indexOf(handle);
      if (index >= 0) handlers.splice(index, 1);
    };
  }, [enabled]);
}
