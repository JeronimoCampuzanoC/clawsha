import { useState, type FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { ApiError, login, register } from "../api";
import { useAuth } from "../auth";

type Mode = "login" | "register";

export default function LoginPage() {
  const [mode, setMode] = useState<Mode>("login");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [info, setInfo] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { signIn } = useAuth();
  const navigate = useNavigate();

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setError(null);
    setInfo(null);
    setBusy(true);
    try {
      if (mode === "login") {
        const result = await login({ username, password });
        signIn(result.token);
        navigate("/files");
      } else {
        await register({ username, password });
        setInfo("Cuenta creada. Ahora puedes iniciar sesión.");
        setMode("login");
        setPassword("");
      }
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Error inesperado");
    } finally {
      setBusy(false);
    }
  }

  function toggleMode() {
    setMode((prev) => (prev === "login" ? "register" : "login"));
    setError(null);
    setInfo(null);
  }

  return (
    <div className="container">
      <div className="card">
        <h2>{mode === "login" ? "Iniciar sesión" : "Crear cuenta"}</h2>
        <form onSubmit={handleSubmit}>
          <label>
            Usuario
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoComplete="username"
              required
            />
          </label>
          <label>
            Contraseña
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete={mode === "login" ? "current-password" : "new-password"}
              required
            />
          </label>
          {error && <div className="error">{error}</div>}
          {info && <div className="badge">{info}</div>}
          <button type="submit" disabled={busy}>
            {busy
              ? "Procesando…"
              : mode === "login"
                ? "Entrar"
                : "Registrarme"}
          </button>
        </form>
        <div className="switch">
          {mode === "login" ? "¿No tienes cuenta?" : "¿Ya tienes cuenta?"}{" "}
          <button type="button" className="link" onClick={toggleMode}>
            {mode === "login" ? "Regístrate" : "Inicia sesión"}
          </button>
        </div>
      </div>
    </div>
  );
}
