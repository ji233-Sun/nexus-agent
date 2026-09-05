import type { ConnectionConfig, Message, RemoteState } from "./types";

interface ApiErrorBody {
  error?: string;
}

export function normalizeEndpoint(value: string): string {
  const url = new URL(value.trim());
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("服务地址必须以 http:// 或 https:// 开头");
  }
  return url.origin;
}

export class NexusApi {
  private readonly endpoint: string;
  private readonly token: string;

  constructor(config: ConnectionConfig) {
    this.endpoint = normalizeEndpoint(config.endpoint);
    this.token = config.token;
  }

  getState(): Promise<RemoteState> {
    return this.request("/api/v1/state");
  }

  getMessages(taskId: string): Promise<Message[]> {
    return this.request(`/api/v1/tasks/${encodeURIComponent(taskId)}/messages`);
  }

  startRun(projectId: string, prompt: string): Promise<void> {
    return this.request("/api/v1/runs", {
      method: "POST",
      body: JSON.stringify({ project_id: projectId, prompt }),
    }).then(() => undefined);
  }

  cancelRun(): Promise<void> {
    return this.request("/api/v1/runs/cancel", { method: "POST" }).then(
      () => undefined,
    );
  }

  eventUrl(): string {
    const url = new URL("/api/v1/events", this.endpoint);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    url.searchParams.set("token", this.token);
    return url.toString();
  }

  private async request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const response = await fetch(new URL(path, this.endpoint), {
      ...init,
      headers: {
        Authorization: `Bearer ${this.token}`,
        "Content-Type": "application/json",
        ...init.headers,
      },
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as ApiErrorBody;
      throw new Error(body.error ?? `请求失败（${response.status}）`);
    }
    return (await response.json()) as T;
  }
}
