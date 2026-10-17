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
  createDir,
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

interface Crumb {
  name: string;
  path: string;
}

/// Breaks "/a/b" into clickable crumbs: root first, then each level with its
/// absolute path, so the breadcrumb can jump straight to any ancestor.
function pathSegments(path: string): Crumb[] {
  const crumbs: Crumb[] = [{ name: "Inicio", path: "/" }];
  let acc = "";
  for (const part of path.split("/").filter(Boolean)) {
    acc = `${acc}/${part}`;
    crumbs.push({ name: part, path: acc });
  }
  return crumbs;
}

export default function FilesPage() {
  const [path, setPath] = useState("/");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadInfo, setUploadInfo] = useState<string | null>(null);
  const [progress, setProgress] = useState(0);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const [dirName, setDirName] = useState("");
  const [creatingDir, setCreatingDir] = useState(false);
  const [dirError, setDirError] = useState<string | null>(null);
  const [dirInfo, setDirInfo] = useState<string | null>(null);

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

  function navigateTo(target: string) {
    setPath(target || "/");
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

  async function handleCreateDir(event: FormEvent) {
    event.preventDefault();
    const name = dirName.trim();
    if (!name) return;
    if (name.includes("/")) {
      setDirError("El nombre no puede contener «/».");
      return;
    }
    setCreatingDir(true);
    setDirError(null);
    setDirInfo(null);
    const target = joinPath(path, name);
    try {
      await createDir(target);
      setDirInfo(`Carpeta «${name}» creada.`);
      setDirName("");
      await load(path);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        handleUnauthorized();
        return;
      }
      setDirError(err instanceof ApiError ? err.message : "Error inesperado");
    } finally {
      setCreatingDir(false);
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

      <div className="drive">
        <aside className="sidebar">
          <div className="card">
            <h2>Crear carpeta</h2>
            <form onSubmit={handleCreateDir}>
              <input
                type="text"
                value={dirName}
                onChange={(e) => {
                  setDirName(e.target.value);
                  setDirError(null);
                  setDirInfo(null);
                }}
                placeholder="nombre-de-carpeta"
                disabled={creatingDir}
              />
              <button type="submit" disabled={!dirName.trim() || creatingDir}>
                {creatingDir ? "Creando…" : "Crear carpeta"}
              </button>
            </form>
            {dirError && <div className="error">{dirError}</div>}
            {dirInfo && <div className="badge">{dirInfo}</div>}
          </div>

          <div className="card">
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
                  {formatSize(selectedFile.size)}
                </div>
              )}
              {uploadError && <div className="error">{uploadError}</div>}
              {uploadInfo && <div className="badge">{uploadInfo}</div>}
              {uploading && (
                <div className="progress" aria-label={`Progreso ${progress}%`}>
                  <div
                    className="progress-bar"
                    style={{ width: `${progress}%` }}
                  >
                    {progress}%
                  </div>
                </div>
              )}
              <button type="submit" disabled={!selectedFile || uploading}>
                {uploading ? "Subiendo…" : "Subir aquí"}
              </button>
            </form>
          </div>
        </aside>

        <section className="content">
          <nav className="breadcrumb" aria-label="Ruta actual">
            {pathSegments(path).map((crumb, index, all) => {
              const isCurrent = index === all.length - 1;
              return (
                <span key={crumb.path} style={{ display: "contents" }}>
                  {index > 0 && <span className="sep">/</span>}
                  <button
                    type="button"
                    className={`crumb${isCurrent ? " current" : ""}`}
                    onClick={() => !isCurrent && navigateTo(crumb.path)}
                    disabled={isCurrent || loading}
                  >
                    {crumb.name}
                  </button>
                </span>
              );
            })}
          </nav>

          <div className="card">
            {error && <div className="error">{error}</div>}

            {!error && entries.length === 0 && !loading && (
              <div className="empty">Esta carpeta está vacía.</div>
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
                  {entries.map((entry) => {
                    const isDir = entry.kind === "directory";
                    return (
                      <tr
                        key={entry.name}
                        onDoubleClick={() =>
                          isDir && navigateTo(joinPath(path, entry.name))
                        }
                        style={{ cursor: isDir ? "pointer" : "default" }}
                      >
                        <td>
                          {isDir ? "📁" : "📄"} {entry.name}
                        </td>
                        <td>
                          <span className="badge">{entry.kind}</span>
                        </td>
                        <td>{isDir ? "—" : formatSize(entry.size)}</td>
                        <td>{formatDate(entry.modified_at)}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </div>
        </section>
      </div>
    </>
  );
}
