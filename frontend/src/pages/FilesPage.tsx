import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type FormEvent,
} from "react";
import { useNavigate } from "react-router-dom";
import {
  ApiError,
  listFiles,
  uploadFile,
  type FileEntry,
} from "../api";
import { useAuth } from "../auth";

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const exp = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  const value = bytes / Math.pow(1024, exp);
  return `${value.toFixed(exp === 0 ? 0 : 1)} ${units[exp]}`;
}

function formatDate(epoch: number): string {
  if (!epoch) return "—";
  const ms = epoch > 1e12 ? epoch : epoch * 1000;
  return new Date(ms).toLocaleString();
}

function joinPath(dir: string, name: string): string {
  const base = dir.endsWith("/") ? dir : `${dir}/`;
  return `${base}${name}`.replace(/\/{2,}/g, "/");
}

export default function FilesPage() {
  const [path, setPath] = useState("/");
  const [inputPath, setInputPath] = useState("/");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadInfo, setUploadInfo] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const { signOut } = useAuth();
  const navigate = useNavigate();

  const handleUnauthorized = useCallback(() => {
    signOut();
    navigate("/login");
  }, [navigate, signOut]);

  const load = useCallback(
    async (target: string) => {
      setLoading(true);
      setError(null);
      try {
        const result = await listFiles(target);
        setEntries(result.entries);
      } catch (err) {
        if (err instanceof ApiError && err.status === 401) {
          handleUnauthorized();
          return;
        }
        setError(err instanceof ApiError ? err.message : "Error inesperado");
      } finally {
        setLoading(false);
      }
    },
    [handleUnauthorized],
  );

  useEffect(() => {
    load(path);
  }, [path, load]);

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setPath(inputPath.trim() || "/");
  }

  function handleFileChange(event: ChangeEvent<HTMLInputElement>) {
    setSelectedFile(event.target.files?.[0] ?? null);
    setUploadError(null);
    setUploadInfo(null);
  }

  async function handleUpload(event: FormEvent) {
    event.preventDefault();
    if (!selectedFile) return;
    setUploading(true);
    setUploadError(null);
    setUploadInfo(null);
    setProgress(0);
    const target = joinPath(path, selectedFile.name);
    try {
      await uploadFile(target, selectedFile, (loaded, total) => {
        setProgress(total > 0 ? Math.round((loaded / total) * 100) : 0);
      });
      setUploadInfo(`«${selectedFile.name}» subido correctamente.`);
      setSelectedFile(null);
      setProgress(0);
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
      await load(path);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        handleUnauthorized();
        return;
      }
      setUploadError(err instanceof ApiError ? err.message : "Error inesperado");
    } finally {
      setUploading(false);
    }
  }

  function handleLogout() {
    signOut();
    navigate("/login");
  }

  return (
    <>
      <header className="app-header">
        <h1>DFSha — Archivos</h1>
        <button className="link" style={{ color: "#fff" }} onClick={handleLogout}>
          Cerrar sesión
        </button>
      </header>
      <div className="container">
        <div className="card" style={{ marginBottom: "1.5rem" }}>
          <h2>Subir archivo</h2>
          <form onSubmit={handleUpload}>
            <input
              ref={fileInputRef}
              type="file"
              onChange={handleFileChange}
              disabled={uploading}
            />
            {selectedFile && (
              <div className="switch">
                Destino: <code>{joinPath(path, selectedFile.name)}</code> (
                {formatSize(selectedFile.size)})
              </div>
            )}
            {uploadError && <div className="error">{uploadError}</div>}
            {uploadInfo && <div className="badge">{uploadInfo}</div>}
            {uploading && (
              <div className="progress" aria-label={`Progreso ${progress}%`}>
                <div className="progress-bar" style={{ width: `${progress}%` }}>
                  {progress}%
                </div>
              </div>
            )}
            <button type="submit" disabled={!selectedFile || uploading}>
              {uploading ? "Subiendo…" : "Subir al directorio actual"}
            </button>
          </form>
        </div>

        <div className="card">
          <form className="toolbar" onSubmit={handleSubmit}>
            <input
              type="text"
              value={inputPath}
              onChange={(e) => setInputPath(e.target.value)}
              placeholder="/ruta"
            />
            <button type="submit" disabled={loading}>
              {loading ? "Cargando…" : "Listar"}
            </button>
          </form>

          {error && <div className="error">{error}</div>}

          {!error && entries.length === 0 && !loading && (
            <div className="empty">No hay archivos en «{path}».</div>
          )}

          {entries.length > 0 && (
            <table>
              <thead>
                <tr>
                  <th>Nombre</th>
                  <th>Tipo</th>
                  <th>Tamaño</th>
                  <th>Modificado</th>
                </tr>
              </thead>
              <tbody>
                {entries.map((entry) => (
                  <tr key={entry.name}>
                    <td>{entry.name}</td>
                    <td>
                      <span className="badge">{entry.kind}</span>
                    </td>
                    <td>
                      {entry.kind === "directory" ? "—" : formatSize(entry.size)}
                    </td>
                    <td>{formatDate(entry.modified_at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </>
  );
}
