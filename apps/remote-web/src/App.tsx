import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { LogOut, Plug, Send, Square } from "lucide-react";

import { NexusApi, normalizeEndpoint } from "./api";
import type {
  ConnectionConfig,
  Message,
  MessageKind,
  MessageRole,
  RemoteState,
  RunStatus,
} from "./types";

const SESSION_ENDPOINT_KEY = "nexus.remote.endpoint";
const SESSION_TOKEN_KEY = "nexus.remote.token";
const REFRESH_DELAY_MS = 100;

function initialConnection(): ConnectionConfig | null {
  const fragment = new URLSearchParams(window.location.hash.slice(1));
  const fragmentToken = fragment.get("token")?.trim();
  if (fragmentToken) {
    const config = {
      endpoint: import.meta.env.DEV ? "http://127.0.0.1:3210" : window.location.origin,
      token: fragmentToken,
    };
    sessionStorage.setItem(SESSION_ENDPOINT_KEY, config.endpoint);
    sessionStorage.setItem(SESSION_TOKEN_KEY, config.token);
    window.history.replaceState(null, "", window.location.pathname + window.location.search);
    return config;
  }

  const endpoint = sessionStorage.getItem(SESSION_ENDPOINT_KEY);
  const token = sessionStorage.getItem(SESSION_TOKEN_KEY);
  return endpoint && token ? { endpoint, token } : null;
}

export default function App() {
  const [connection, setConnection] = useState<ConnectionConfig | null>(initialConnection);

  const connect = (config: ConnectionConfig) => {
    sessionStorage.setItem(SESSION_ENDPOINT_KEY, config.endpoint);
    sessionStorage.setItem(SESSION_TOKEN_KEY, config.token);
    setConnection(config);
  };

  const disconnect = () => {
    sessionStorage.removeItem(SESSION_ENDPOINT_KEY);
    sessionStorage.removeItem(SESSION_TOKEN_KEY);
    setConnection(null);
  };

  return connection ? (
    <RemoteWorkspace connection={connection} onDisconnect={disconnect} />
  ) : (
    <ConnectionScreen onConnect={connect} />
  );
}

function ConnectionScreen({ onConnect }: { onConnect: (config: ConnectionConfig) => void }) {
  const localDefault = import.meta.env.DEV
    ? "http://127.0.0.1:3210"
    : window.location.origin;
  const [endpoint, setEndpoint] = useState(
    sessionStorage.getItem(SESSION_ENDPOINT_KEY) ?? localDefault,
  );
  const [token, setToken] = useState("");
  const [error, setError] = useState("");

  const submit = (event: FormEvent) => {
    event.preventDefault();
    try {
      const normalized = normalizeEndpoint(endpoint);
      if (!token.trim()) {
        throw new Error("请输入 Nexus 中显示的访问令牌");
      }
      onConnect({ endpoint: normalized, token: token.trim() });
    } catch (reason) {
      setError(errorMessage(reason));
    }
  };

  return (
    <main className="connection-page">
      <form className="connection-card" onSubmit={submit}>
        <div className="brand-mark">N</div>
        <div>
          <p className="eyebrow">NEXUS REMOTE</p>
          <h1>连接你的本地 Nexus</h1>
        </div>
        <label>
          服务地址
          <input
            value={endpoint}
            onChange={(event) => setEndpoint(event.target.value)}
            placeholder="https://nexus.example.com"
            inputMode="url"
            autoCapitalize="none"
          />
        </label>
        <label>
          访问令牌
          <input
            value={token}
            onChange={(event) => setToken(event.target.value)}
            placeholder="••••••••"
            type="password"
            autoComplete="off"
          />
        </label>
        {error && <p className="error-banner">{error}</p>}
        <button className="primary-button" type="submit">
          <Plug aria-hidden="true" size={16} />
          连接
        </button>
      </form>
    </main>
  );
}

function RemoteWorkspace({
  connection,
  onDisconnect,
}: {
  connection: ConnectionConfig;
  onDisconnect: () => void;
}) {
  const api = useMemo(() => new NexusApi(connection), [connection]);
  const [state, setState] = useState<RemoteState | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("");
  const [connected, setConnected] = useState(false);
  const [loading, setLoading] = useState(true);
  const [actionPending, setActionPending] = useState(false);
  const [error, setError] = useState("");
  const refreshTimer = useRef<number | null>(null);
  const selectedTaskRef = useRef<string | null>(null);

  selectedTaskRef.current = selectedTaskId;

  const refresh = useCallback(async () => {
    try {
      const nextState = await api.getState();
      setState(nextState);
      const taskId = selectedTaskRef.current;
      if (taskId) {
        setMessages(await api.getMessages(taskId));
      }
      setError("");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, [api]);

  const scheduleRefresh = useCallback(() => {
    if (refreshTimer.current !== null) {
      return;
    }
    refreshTimer.current = window.setTimeout(() => {
      refreshTimer.current = null;
      void refresh();
    }, REFRESH_DELAY_MS);
  }, [refresh]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    let retryTimer: number | null = null;
    let socket: WebSocket | null = null;

    const open = () => {
      socket = new WebSocket(api.eventUrl());
      socket.onopen = () => {
        setConnected(true);
        setError("");
      };
      socket.onmessage = scheduleRefresh;
      socket.onerror = () => socket?.close();
      socket.onclose = () => {
        setConnected(false);
        if (!disposed) {
          retryTimer = window.setTimeout(open, 1500);
        }
      };
    };

    open();
    return () => {
      disposed = true;
      if (retryTimer !== null) {
        window.clearTimeout(retryTimer);
      }
      socket?.close();
    };
  }, [api, scheduleRefresh]);

  useEffect(() => () => {
    if (refreshTimer.current !== null) {
      window.clearTimeout(refreshTimer.current);
    }
  }, []);

  useEffect(() => {
    if (!state) {
      return;
    }
    setSelectedProjectId((current) => {
      if (state.projects.some((project) => project.id === current)) {
        return current;
      }
      if (state.projects.some((project) => project.id === state.selected_project_id)) {
        return state.selected_project_id;
      }
      return state.projects[0]?.id ?? null;
    });
  }, [state]);

  const projectTasks = useMemo(
    () => state?.tasks.filter((task) => task.project_id === selectedProjectId) ?? [],
    [selectedProjectId, state],
  );

  useEffect(() => {
    setSelectedTaskId((current) => {
      if (projectTasks.some((task) => task.id === current)) {
        return current;
      }
      const preferred = [state?.active_task_id, state?.selected_task_id].find((taskId) =>
        projectTasks.some((task) => task.id === taskId),
      );
      return preferred ?? projectTasks[0]?.id ?? null;
    });
  }, [projectTasks, state?.active_task_id, state?.selected_task_id]);

  useEffect(() => {
    if (!selectedTaskId) {
      setMessages([]);
      return;
    }
    api.getMessages(selectedTaskId).then(setMessages).catch((reason) => {
      setError(errorMessage(reason));
    });
  }, [api, selectedTaskId]);

  const run = async (event: FormEvent) => {
    event.preventDefault();
    if (!selectedProjectId || !prompt.trim() || actionPending) {
      return;
    }
    setActionPending(true);
    try {
      await api.startRun(selectedProjectId, prompt.trim());
      setPrompt("");
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setActionPending(false);
    }
  };

  const cancel = async () => {
    setActionPending(true);
    try {
      await api.cancelRun();
      await refresh();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setActionPending(false);
    }
  };

  const selectedTask = projectTasks.find((task) => task.id === selectedTaskId);
  const canRun = Boolean(
    state?.harness_ready &&
      !state.active_run_id &&
      selectedProjectId &&
      prompt.trim() &&
      !actionPending,
  );
  const showStreaming = state?.active_task_id === selectedTaskId && state.streaming_text;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand-row">
          <div className="brand-mark small">N</div>
          <div>
            <strong>Nexus</strong>
            <span>Remote</span>
          </div>
        </div>
        <label className="project-picker">
          项目
          <select
            value={selectedProjectId ?? ""}
            onChange={(event) => setSelectedProjectId(event.target.value || null)}
          >
            {state?.projects.map((project) => (
              <option key={project.id} value={project.id}>{project.display_name}</option>
            ))}
          </select>
        </label>
        <div className="section-title">
          <span>Nexus 会话</span>
          <span>{projectTasks.length}</span>
        </div>
        <nav className="task-list" aria-label="Nexus 会话">
          {projectTasks.map((task) => (
            <button
              className={task.id === selectedTaskId ? "task-row selected" : "task-row"}
              key={task.id}
              onClick={() => setSelectedTaskId(task.id)}
              type="button"
            >
              <span>{task.title}</span>
              <small><i className={`status-dot ${task.status}`} />{statusLabel(task.status)}</small>
            </button>
          ))}
          {!projectTasks.length && <p className="empty-sidebar">这个项目还没有 Nexus 会话。</p>}
        </nav>
        <div className="connection-status">
          <div>
            <i className={connected ? "status-dot running" : "status-dot failed"} />
            {connected ? "实时连接" : "正在重连"}
          </div>
          <button className="text-button" onClick={onDisconnect} type="button">
            <LogOut aria-hidden="true" size={13} />
            断开
          </button>
        </div>
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div>
            <p className="eyebrow">{selectedTask ? "任务时间线" : "REMOTE CONTROL"}</p>
            <h2>{selectedTask?.title ?? "新建远程任务"}</h2>
          </div>
          <div className="runtime-status">
            <span className={state?.harness_ready ? "ready" : "not-ready"}>
              {state?.harness === "codex" ? "Codex CLI" : "Claude Code"}
            </span>
            <span>{state?.effort ?? "—"}</span>
          </div>
        </header>

        {error && <div className="error-banner workspace-error">{error}</div>}

        <section className="timeline" aria-live="polite">
          {loading && <EmptyState title="正在连接 Nexus" detail="读取本地状态和会话记录…" />}
          {!loading && messages.map((message) => <MessageCard key={message.id} message={message} />)}
          {showStreaming && (
            <article className="message-card assistant streaming">
              <MessageHeader role="assistant" kind="text" streaming />
              <p>{state.streaming_text}</p>
            </article>
          )}
          {!loading && !messages.length && !showStreaming && (
            <EmptyState
              title={selectedProjectId ? "准备开始新任务" : "请先在桌面端打开项目"}
              detail={selectedProjectId ? "此项目尚无会话。" : "Nexus 中暂无已登记项目。"}
            />
          )}
        </section>

        <form className="composer" onSubmit={run}>
          <textarea
            value={prompt}
            onChange={(event) => setPrompt(event.target.value)}
            placeholder="描述你希望 Agent 完成的工作…"
            rows={3}
            disabled={Boolean(state?.active_run_id)}
          />
          <div className="composer-footer">
            <span>{state?.status ?? "正在连接…"}</span>
            {state?.active_run_id ? (
              <button className="danger-button" type="button" onClick={cancel} disabled={actionPending}>
                <Square aria-hidden="true" size={14} />
                取消运行
              </button>
            ) : (
              <button className="primary-button compact" type="submit" disabled={!canRun}>
                <Send aria-hidden="true" size={14} />
                {actionPending ? "发送中…" : "发送"}
              </button>
            )}
          </div>
        </form>
      </main>
    </div>
  );
}

function MessageCard({ message }: { message: Message }) {
  return (
    <article className={`message-card ${message.role} ${message.kind}`}>
      <MessageHeader role={message.role} kind={message.kind} />
      <p>{message.content}</p>
    </article>
  );
}

function MessageHeader({
  role,
  kind,
  streaming = false,
}: {
  role: MessageRole;
  kind: MessageKind;
  streaming?: boolean;
}) {
  const roleName: Record<MessageRole, string> = {
    user: "You",
    assistant: "Agent",
    tool: "Tool",
    system: "System",
  };
  return (
    <header>
      <i />
      <strong>{roleName[role]}</strong>
      <span>{streaming ? "正在生成" : kindLabel(kind)}</span>
    </header>
  );
}

function EmptyState({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="empty-state">
      <div className="empty-icon">◇</div>
      <h3>{title}</h3>
      <p>{detail}</p>
    </div>
  );
}

function statusLabel(status: RunStatus): string {
  const labels: Record<RunStatus, string> = {
    starting: "启动中",
    running: "运行中",
    cancelling: "取消中",
    completed: "已完成",
    failed: "失败",
    cancelled: "已取消",
    interrupted: "已中断",
  };
  return labels[status];
}

function kindLabel(kind: MessageKind): string {
  const labels: Record<MessageKind, string> = {
    text: "消息",
    tool_call: "工具调用",
    tool_result: "工具结果",
    status: "状态",
    error: "错误",
  };
  return labels[kind];
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : "发生未知错误";
}
