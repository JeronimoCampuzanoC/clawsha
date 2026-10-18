import { useEffect, useState } from "react";
import {
  ApiError,
  downloadFile,
  fetchFileBlobUrl,
  previewKind,
  PREVIEW_MAX_BYTES,
  type FileEntry,
} from "../api";

interface Props {
  path: string;
  entry: FileEntry;
  onClose: () => void;
  onUnauthorized: () => void;
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const exp = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  return `${(bytes / Math.pow(1024, exp)).toFixed(exp === 0 ? 0 : 1)} ${units[exp]}`;
}

export default function FilePreview({
  path,
  entry,
  onClose,
  onUnauthorized,
}: Props) {
  const kind = previewKind(entry.name);
  const tooLarge = entry.size > PREVIEW_MAX_BYTES;
  const canPreview = kind !== null && !tooLarge;

  const [blobUrl, setBlobUrl] = useState<string | null>(null);
  const [textContent, setTextContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [downloading, setDownloading] = useState(false);

  useEffect(() => {
    if (!canPreview) return;

    let revoked = false;
    let url: string | null = null;
    setLoading(true);
    setError(null);

    fetchFileBlobUrl(path)
      .then(async (objectUrl) => {
        if (revoked) {
          URL.revokeObjectURL(objectUrl);
          return;
        }
        url = objectUrl;
        setBlobUrl(objectUrl);
        if (kind === "text") {
          const res = await fetch(objectUrl);
          setTextContent(await res.text());
        }
      })
      .catch((err) => {
        if (err instanceof ApiError && err.status === 401) {
          onUnauthorized();
          return;
        }
        setError(err instanceof ApiError ? err.message : "Error inesperado");
      })
      .finally(() => {
        if (!revoked) setLoading(false);
      });

    return () => {
      revoked = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [path, kind, canPreview, onUnauthorized]);

  async function handleDownload() {
    setDownloading(true);
    setError(null);
    try {
      await downloadFile(path, entry.name);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        onUnauthorized();
        return;
      }
      setError(err instanceof ApiError ? err.message : "Error inesperado");
    } finally {
      setDownloading(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <header className="modal-header">
          <h3>
            📄 {entry.name} <span className="muted">({formatSize(entry.size)})</span>
          </h3>
          <button className="link" onClick={onClose}>
            Cerrar
          </button>
        </header>

        <div className="modal-body">
          {error && <div className="error">{error}</div>}

          {!canPreview && (
            <div className="empty">
              {tooLarge
                ? `El archivo es demasiado grande para previsualizar (máx. ${formatSize(
                    PREVIEW_MAX_BYTES,
                  )}). Descárgalo para verlo.`
                : "Este tipo de archivo no admite vista previa. Descárgalo para verlo."}
            </div>
          )}

          {canPreview && loading && <div className="empty">Cargando vista previa…</div>}

          {canPreview && !loading && blobUrl && (
            <>
              {kind === "image" && (
                <img className="preview-media" src={blobUrl} alt={entry.name} />
              )}
              {kind === "pdf" && (
                <iframe className="preview-frame" src={blobUrl} title={entry.name} />
              )}
              {kind === "video" && (
                <video className="preview-media" src={blobUrl} controls />
              )}
              {kind === "audio" && <audio src={blobUrl} controls />}
              {kind === "text" && textContent !== null && (
                <pre className="preview-text">{textContent}</pre>
              )}
            </>
          )}
        </div>

        <footer className="modal-footer">
          <button onClick={handleDownload} disabled={downloading}>
            {downloading ? "Descargando…" : "Descargar"}
          </button>
        </footer>
      </div>
    </div>
  );
}
