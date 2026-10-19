import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  ApiError,
  clusterStatus,
  type ClusterStatus,
  type WorkerState,
} from "../api";
import { useAuth } from "../auth";

/// Same pace as the workers' heartbeats: a dead worker or a repair shows up
/// within a few seconds.
const REFRESH_MS = 3_000;

const STATE_LABEL: Record<WorkerState, string> = {
  alive: "Vivo",
  suspect: "Sospechoso",
  dead: "Muerto",
  unknown: "Desconocido",
};

function formatBytes(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function secondsAgo(ms: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  return `hace ${seconds} s`;
}

export default function ClusterPage() {
  const { signOut } = useAuth();
  const navigate = useNavigate();
  const [status, setStatus] = useState<ClusterStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    let cancelled = false;

    async function refresh() {
      try {
        const next = await clusterStatus();
        if (!cancelled) {
          setStatus(next);
          setError(null);
          setNow(Date.now());
        }
      } catch (err) {
        if (cancelled) return;
        if (err instanceof ApiError && err.status === 401) {
          signOut();
          navigate("/login");
          return;
        }
        // Keep the last numbers on screen: during a failover the gateway
        // answers 503 for a few seconds and that is part of the demo.
        setError(err instanceof ApiError ? err.message : "Error inesperado");
      }
    }

    refresh();
    const timer = window.setInterval(refresh, REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [navigate, signOut]);

  function handleLogout() {
    signOut();
    navigate("/login");
  }

  const workers = status?.workers ?? [];
  const alive = workers.filter((w) => w.state === "alive").length;

  return (
    <>
      <header className="app-header">
        <h1>DFSha — Clúster</h1>
        <nav className="header-nav">
          <button className="link" style={{ color: "#fff" }} onClick={() => navigate("/files")}>
            Archivos
          </button>
          <button className="link" style={{ color: "#fff" }} onClick={handleLogout}>
            Cerrar sesión
          </button>
        </nav>
      </header>

      <main className="cluster">
        {error && <p className="error">{error} — reintentando…</p>}

        {status && (
          <>
            <section className="stats">
              <div className="card stat">
                <span className="stat-label">Master líder</span>
                <span className="stat-value">{status.master_id}</span>
                <span className="muted">
                  fencing {status.fencing}
                  {status.safe_mode && " · modo seguro"}
                </span>
              </div>
              <div className="card stat">
                <span className="stat-label">Workers vivos</span>
                <span className="stat-value">
                  {alive} / {workers.length}
                </span>
              </div>
              <div className="card stat">
                <span className="stat-label">Bloques</span>
                <span className="stat-value">{status.total_blocks}</span>
              </div>
              <div
                className={`card stat ${status.under_replicated_blocks > 0 ? "stat-alert" : "stat-ok"}`}
              >
                <span className="stat-label">Sub-replicados</span>
                <span className="stat-value">{status.under_replicated_blocks}</span>
                <span className="muted">
                  {status.under_replicated_blocks > 0 ? "re-replicando" : "todos con 2 copias"}
                </span>
              </div>
            </section>

            <section className="card">
              <h2>Workers</h2>
              {workers.length === 0 ? (
                <p className="empty">Ningún worker se ha registrado todavía.</p>
              ) : (
                <table>
                  <thead>
                    <tr>
                      <th>Worker</th>
                      <th>AZ</th>
                      <th>Estado</th>
                      <th>Uso</th>
                      <th>Bloques</th>
                      <th>Último heartbeat</th>
                    </tr>
                  </thead>
                  <tbody>
                    {workers.map((w) => {
                      const percent =
                        w.quota_bytes > 0 ? (w.used_bytes * 100) / w.quota_bytes : 0;
                      return (
                        <tr key={w.id}>
                          <td>
                            <strong>{w.id}</strong>
                            <div className="muted">{w.addr}</div>
                          </td>
                          <td>{w.az}</td>
                          <td>
                            <span className={`badge state-${w.state}`}>
                              {STATE_LABEL[w.state]}
                            </span>
                          </td>
                          <td className="usage">
                            <div className="progress">
                              <div
                                className="progress-bar"
                                style={{ width: `${Math.min(100, percent)}%` }}
                              >
                                {percent.toFixed(0)}%
                              </div>
                            </div>
                            <div className="muted">
                              {formatBytes(w.used_bytes)} de {formatBytes(w.quota_bytes)}
                            </div>
                          </td>
                          <td>{w.block_count}</td>
                          <td>{secondsAgo(w.last_heartbeat, now)}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )}
            </section>
          </>
        )}

        {!status && !error && <p className="empty">Cargando…</p>}
      </main>
    </>
  );
}
