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
  moveEntry,
  removeEntry,
  uploadFile,
  type FileEntry,
} from "../api";
import { useAuth } from "../auth";
import FilePreview from "../components/FilePreview";

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

  const [preview, setPreview] = useState<FileEntry | null>(null);

  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const [moving, setMoving] = useState<string | null>(null);
  const [moveError, setMoveError] = useState<string | null>(null);

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
      const result = await uploadFile(target, selectedFile, (loaded, total) => {
        setProgress(total > 0 ? Math.round((loaded / total) * 100) : 0);
      });
      // If the name was taken, the master saved it as "name (1).ext".
      const savedAs = result.path.split("/").pop() || selectedFile.name;
      setUploadInfo(
        savedAs === selectedFile.name
          ? `«${savedAs}» subido correctamente.`
          : `«${selectedFile.name}» ya existía: se subió como «${savedAs}».`,
      );
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

  async function handleDelete(entry: FileEntry) {
    const isDir = entry.kind === "directory";
    const target = joinPath(path, entry.name);
    const label = isDir ? "la carpeta" : "el archivo";
    if (!window.confirm(`¿Eliminar ${label} «${entry.name}»?`)) {
      return;
    }

    setDeleting(target);
    setDeleteError(null);
    try {
      await removeEntry(target, false);
      await load(path);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        handleUnauthorized();
        return;
      }
      // 409 on a directory means it is not empty: offer a recursive delete.
      if (err instanceof ApiError && err.status === 409 && isDir) {
        const confirmed = window.confirm(
          `La carpeta «${entry.name}» no está vacía. ¿Eliminarla con todo su contenido?`,
        );
        if (!confirmed) {
          return;
        }
        try {
          await removeEntry(target, true);
          await load(path);
        } catch (recursiveErr) {
          if (recursiveErr instanceof ApiError && recursiveErr.status === 401) {
            handleUnauthorized();
            return;
          }
          setDeleteError(
            recursiveErr instanceof ApiError
              ? recursiveErr.message
              : "Error inesperado",
          );
        }
        return;
      }
      setDeleteError(err instanceof ApiError ? err.message : "Error inesperado");
    } finally {
      setDeleting(null);
    }
  }

  function handleLogout() {
    signOut();
    navigate("/login");
  }

  async function handleMove(entry: FileEntry) {
    const from = joinPath(path, entry.name);
    const input = window.prompt(
      "Nueva ruta (absoluta, incluyendo el nombre):",
      from,
    );
    if (input === null) {
      return;
    }
    const to = input.trim();
    if (!to || to === from) {
      return;
    }
    if (!to.startsWith("/")) {
      setMoveError("La ruta de destino debe ser absoluta (empezar con «/»).");
      return;
    }

    setMoving(from);
    setMoveError(null);
    try {
      await moveEntry(from, to);
      await load(path);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        handleUnauthorized();
        return;
      }
      setMoveError(err instanceof ApiError ? err.message : "Error inesperado");
    } finally {
      setMoving(null);
    }
  }

  return (
    <>
      <header className="app-header">
        <h1>DClawha — Archivos</h1>
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
            {deleteError && <div className="error">{deleteError}</div>}
            {moveError && <div className="error">{moveError}</div>}

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
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {entries.map((entry) => {
                    const isDir = entry.kind === "directory";
                    return (
                      <tr
                        key={entry.name}
                        onDoubleClick={() =>
                          isDir
                            ? navigateTo(joinPath(path, entry.name))
                            : setPreview(entry)
                        }
                        style={{ cursor: "pointer" }}
                      >
                        <td>
                          {isDir ? "📁" : "📄"} {entry.name}
                        </td>
                        <td>
                          <span className="badge">{entry.kind}</span>
                        </td>
                        <td>{isDir ? "—" : formatSize(entry.size)}</td>
                        <td>{formatDate(entry.modified_at)}</td>
                        <td>
                          <div className="actions">
                            {!isDir && (
                              <button
                                type="button"
                                className="action-btn"
                                onClick={() => setPreview(entry)}
                              >
                                Abrir
                              </button>
                            )}
                            <button
                              type="button"
                              className="action-btn"
                              onClick={() => handleMove(entry)}
                              disabled={moving === joinPath(path, entry.name)}
                            >
                              {moving === joinPath(path, entry.name)
                                ? "Moviendo…"
                                : "Renombrar/Mover"}
                            </button>
                            <button
                              type="button"
                              className="action-btn danger"
                              onClick={() => handleDelete(entry)}
                              disabled={deleting === joinPath(path, entry.name)}
                            >
                              {deleting === joinPath(path, entry.name)
                                ? "Eliminando…"
                                : "Eliminar"}
                            </button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </div>
        </section>
      </div>

      {preview && (
        <FilePreview
          path={joinPath(path, preview.name)}
          entry={preview}
          onClose={() => setPreview(null)}
          onUnauthorized={handleUnauthorized}
        />
      )}
    </>
  );
}
