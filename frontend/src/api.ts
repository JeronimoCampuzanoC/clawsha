const BASE_URL = (import.meta.env.VITE_API_BASE_URL ?? "").replace(/\/$/, "");

const TOKEN_KEY = "dfsha.token";

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function setToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token);
}

export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY);
}

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
    this.name = "ApiError";
  }
}

interface Credentials {
  username: string;
  password: string;
}

export interface LoginResult {
  user_id: string;
  token: string;
}

export interface RegisterResult {
  user_id: string;
}

export interface FileEntry {
  name: string;
  kind: string;
  size: number;
  modified_at: number;
}

interface DirectoryListing {
  entries: FileEntry[];
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  const token = getToken();
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }

  let response: Response;
  try {
    response = await fetch(`${BASE_URL}${path}`, { ...init, headers });
  } catch (cause) {
    throw new ApiError(0, "No se pudo conectar con el gateway");
  }

  if (!response.ok) {
    throw new ApiError(response.status, errorMessage(response.status));
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

function errorMessage(status: number): string {
  switch (status) {
    case 400:
      return "Datos inválidos";
    case 401:
      return "Credenciales inválidas o sesión expirada";
    case 404:
      return "No encontrado";
    case 409:
      return "El recurso ya existe";
    case 503:
      return "Servicio no disponible";
    default:
      return `Error del servidor (${status})`;
  }
}

export function register(credentials: Credentials): Promise<RegisterResult> {
  return request<RegisterResult>("/api/v1/auth/register", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(credentials),
  });
}

export function login(credentials: Credentials): Promise<LoginResult> {
  return request<LoginResult>("/api/v1/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(credentials),
  });
}

export function listFiles(path = "/"): Promise<DirectoryListing> {
  const query = new URLSearchParams({ path });
  return request<DirectoryListing>(`/api/v1/fs?${query.toString()}`, {
    method: "GET",
  });
}

export interface CreateDirResult {
  path: string;
}

export function createDir(path: string): Promise<CreateDirResult> {
  const query = new URLSearchParams({ path });
  return request<CreateDirResult>(`/api/v1/dirs?${query.toString()}`, {
    method: "POST",
  });
}

export interface UploadResult {
  path: string;
  inode_id: string;
  version_no: number;
  size: number;
  block_count: number;
  modified_at: number;
}

export function uploadFile(
  path: string,
  file: File,
  onProgress?: (loaded: number, total: number) => void,
): Promise<UploadResult> {
  const query = new URLSearchParams({ path });
  const url = `${BASE_URL}/api/v1/files?${query.toString()}`;

  return new Promise<UploadResult>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", url);
    xhr.responseType = "text";

    const token = getToken();
    if (token) {
      xhr.setRequestHeader("Authorization", `Bearer ${token}`);
    }
    xhr.setRequestHeader("Content-Type", "application/octet-stream");

    if (onProgress) {
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) {
          onProgress(event.loaded, event.total);
        }
      };
    }

    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(xhr.responseText ? JSON.parse(xhr.responseText) : undefined);
        } catch {
          reject(new ApiError(xhr.status, "Respuesta inválida del servidor"));
        }
      } else {
        reject(new ApiError(xhr.status, errorMessage(xhr.status)));
      }
    };

    xhr.onerror = () => reject(new ApiError(0, "No se pudo conectar con el gateway"));
    xhr.onabort = () => reject(new ApiError(0, "Subida cancelada"));

    xhr.send(file);
  });
}
