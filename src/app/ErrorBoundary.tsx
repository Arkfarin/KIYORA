import { Component } from "react";
import type { ReactNode } from "react";
export class ErrorBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? (
      <div className="boot-screen" role="alert">
        <span className="wordmark">KIYORA</span>
        <h1>Не удалось открыть интерфейс</h1>
        <p>Ваш Vault и музыкальные файлы не изменены.</p>
        <button onClick={() => location.reload()}>
          Перезапустить интерфейс
        </button>
      </div>
    ) : (
      this.props.children
    );
  }
}
