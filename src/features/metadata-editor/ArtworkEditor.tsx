import { useEffect, useState } from "react";
import { Crop, ImagePlus, X } from "lucide-react";
import { call, errorOf } from "../../backend";
import { artworkUrl } from "../../services/artwork";
import { IconButton } from "../../components/ui/Primitives";
export function ArtworkEditor({
  bytes,
  initialUrl,
  onChange,
  disabled,
}: {
  bytes: number[] | null;
  initialUrl?: string;
  onChange: (bytes: number[] | null) => void;
  disabled: boolean;
}) {
  const [url, setUrl] = useState(initialUrl);
  const [source, setSource] = useState<number[] | null>(null);
  const [cropUrl, setCropUrl] = useState<string>();
  const [size, setSize] = useState({ width: 1, height: 1 });
  const [zoom, setZoom] = useState(1);
  const [x, setX] = useState(50);
  const [y, setY] = useState(50);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    if (!bytes) {
      setUrl(initialUrl);
      return;
    }
    const next = URL.createObjectURL(
      new Blob([Uint8Array.from(bytes)], { type: "image/png" }),
    );
    setUrl(next);
    return () => URL.revokeObjectURL(next);
  }, [bytes, initialUrl]);
  useEffect(() => {
    if (!source) {
      setCropUrl(undefined);
      return;
    }
    const next = URL.createObjectURL(
      new Blob([Uint8Array.from(source)], { type: "image/png" }),
    );
    setCropUrl(next);
    return () => URL.revokeObjectURL(next);
  }, [source]);
  const cropSize = Math.max(
    1,
    Math.floor(Math.min(size.width, size.height) / zoom),
  );
  const left = Math.floor(((size.width - cropSize) * x) / 100);
  const top = Math.floor(((size.height - cropSize) * y) / 100);
  return (
    <div className="artwork-editor">
      <div className="artwork-preview">
        {url ? (
          <img src={url} alt="Предварительный просмотр обложки" />
        ) : (
          <ImagePlus size={48} strokeWidth={1} />
        )}
      </div>
      <div className="stack">
        <h3>Обложка</h3>
        <p className="muted">
          JPG, PNG или WebP, до 8 МБ. Изображение проверяется и обрезается в
          отдельную копию.
        </p>
        <label className="file-button">
          <ImagePlus />
          Выбрать изображение
          <input
            type="file"
            accept="image/jpeg,image/png,image/webp"
            disabled={disabled || busy}
            onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (!file) return;
              if (file.size > 8 * 1024 * 1024) {
                setError("Изображение должно быть не больше 8 МБ.");
                return;
              }
              setBusy(true);
              setError("");
              void file
                .arrayBuffer()
                .then((buffer) =>
                  call("prepare_artwork", {
                    bytes: Array.from(new Uint8Array(buffer)),
                    crop: null,
                  }),
                )
                .then((art) => {
                  if (art) {
                    setSource(art.bytes);
                    setZoom(1);
                    setX(50);
                    setY(50);
                  }
                })
                .catch((e) => setError(errorOf(e).message))
                .finally(() => setBusy(false));
            }}
          />
        </label>
        <button
          disabled={disabled || !url || busy}
          onClick={() => onChange(null)}
        >
          <X />
          Удалить обложку
        </button>
        {error && <p role="alert">{error}</p>}
        {busy && <p role="status">Обработка изображения…</p>}
      </div>
      {cropUrl && source && (
        <div className="crop-editor">
          <div className="section-heading">
            <h3>Квадратная обложка</h3>
            <IconButton
              label="Отменить обрезку"
              onClick={() => setSource(null)}
            >
              <X />
            </IconButton>
          </div>
          <div className="crop-preview">
            <img
              src={cropUrl}
              alt="Предварительный просмотр обрезки"
              onLoad={(e) =>
                setSize({
                  width: e.currentTarget.naturalWidth,
                  height: e.currentTarget.naturalHeight,
                })
              }
              style={{
                width: `${(size.width / cropSize) * 100}%`,
                maxWidth: "none",
                transform: `translate(${(-left / size.width) * 100}%, ${(-top / size.height) * 100}%)`,
              }}
            />
          </div>
          <label>
            Масштаб
            <input
              type="range"
              min={1}
              max={4}
              step={0.01}
              value={zoom}
              onChange={(e) => setZoom(Number(e.target.value))}
            />
          </label>
          <label>
            По горизонтали
            <input
              type="range"
              min={0}
              max={100}
              value={x}
              onChange={(e) => setX(Number(e.target.value))}
            />
          </label>
          <label>
            По вертикали
            <input
              type="range"
              min={0}
              max={100}
              value={y}
              onChange={(e) => setY(Number(e.target.value))}
            />
          </label>
          <button
            className="primary"
            disabled={busy || disabled}
            onClick={() => {
              setBusy(true);
              void call("prepare_artwork", {
                bytes: source,
                crop: { x: left, y: top, size: cropSize },
              })
                .then((art) => {
                  if (art) {
                    onChange(art.bytes);
                    setSource(null);
                  }
                })
                .catch((e) => setError(errorOf(e).message))
                .finally(() => setBusy(false));
            }}
          >
            <Crop />
            Применить обрезку
          </button>
        </div>
      )}
    </div>
  );
}
export { artworkUrl };
