import { useEffect, useId, useRef } from "react";
import type { ButtonHTMLAttributes, ReactNode } from "react";
import { useBack } from "../../app/back";
import { X, ChevronDown, LoaderCircle, Music2 } from "lucide-react";

export function IconButton({
  label,
  children,
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      className={`icon-button ${className}`}
      {...props}
    >
      {children}
    </button>
  );
}
export function Modal({
  title,
  children,
  onClose,
  wide = false,
  fullScreen = false,
  className = "",
  headingAction,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  wide?: boolean;
  fullScreen?: boolean;
  className?: string;
  headingAction?: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const close = useRef(onClose);
  close.current = onClose;
  useBack(() => {
    close.current();
    return true;
  });
  useEffect(() => {
    const node = ref.current;
    node?.showModal();
    return () => node?.close();
  }, []);
  return (
    <dialog
      ref={ref}
      className={`modal ${wide ? "modal-wide" : ""} ${fullScreen ? "modal-fullscreen" : ""} ${className}`}
      aria-labelledby={titleId}
      onCancel={(e) => {
        e.preventDefault();
        close.current();
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget) {
          const r = e.currentTarget.getBoundingClientRect();
          if (
            e.clientX < r.left ||
            e.clientX > r.right ||
            e.clientY < r.top ||
            e.clientY > r.bottom
          )
            close.current();
        }
      }}
    >
      <div className="modal-heading">
        {fullScreen && (
          <IconButton label="Свернуть плеер" onClick={onClose}>
            <ChevronDown />
          </IconButton>
        )}
        <h2 id={titleId}>{title}</h2>
        {headingAction}
        {!fullScreen && (
          <IconButton label="Закрыть" onClick={onClose}>
            <X />
          </IconButton>
        )}
      </div>
      {children}
    </dialog>
  );
}
export function Empty({
  title,
  children,
  action,
}: {
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty-state">
      <Music2 size={36} strokeWidth={1.2} aria-hidden="true" />
      <h2>{title}</h2>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}
export function Loading({ label = "Загрузка…" }: { label?: string }) {
  return (
    <div className="loading" role="status">
      <LoaderCircle className="spin" aria-hidden="true" />
      {label}
    </div>
  );
}
export function PageHeading({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string;
  children?: ReactNode;
}) {
  return (
    <div className="page-heading">
      <div>
        <h1>{title}</h1>
        {subtitle && <p>{subtitle}</p>}
      </div>
      <div className="actions">{children}</div>
    </div>
  );
}
export function Confirm({
  title,
  children,
  confirmLabel,
  busy,
  onConfirm,
  onClose,
}: {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  busy: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Modal title={title} onClose={onClose}>
      <p className="dialog-copy">{children}</p>
      <div className="dialog-actions">
        <button onClick={onClose}>Отмена</button>
        <button className="danger" disabled={busy} onClick={onConfirm}>
          {busy ? "Выполняется…" : confirmLabel}
        </button>
      </div>
    </Modal>
  );
}
