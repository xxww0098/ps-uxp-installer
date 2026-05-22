import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { ask, open } from "@tauri-apps/plugin-dialog";

const DRAG_RESET_MS = 1500;

interface InstallResult {
  message: string;
}

interface PhotoshopInstall {
  name: string;
  version: string;
  path: string;
}

interface UxpPlugin {
  id: string;
  name: string;
  version: string;
  host_version: string;
  source: string;
  path: string;
}

interface PsStatus {
  upia_path: string | null;
  photoshop_versions: PhotoshopInstall[];
  installed_uxp: UxpPlugin[];
}

interface DragDropPayload {
  paths?: string[];
  position?: { x: number; y: number };
}

type Status = "idle" | "checking" | "installing" | "success" | "error";

export default function App() {
  const [status, setStatus] = useState<Status>("idle");
  const [message, setMessage] = useState("");
  const [upiaFound, setUpiaFound] = useState<boolean | null>(null);
  const [psStatus, setPsStatus] = useState<PsStatus | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [dragging, setDragging] = useState(false);
  const isBusy = status === "installing";

  const refreshSeqRef = useRef(0);
  const photoshopFoundRef = useRef<boolean | null>(null);
  const isBusyRef = useRef(false);
  const photoshopFound = psStatus ? psStatus.photoshop_versions.length > 0 : null;
  photoshopFoundRef.current = photoshopFound;
  isBusyRef.current = isBusy;

  const refreshPsStatus = useCallback(() => {
    const seq = ++refreshSeqRef.current;
    setRefreshing(true);
    invoke<PsStatus>("get_ps_status")
      .then((result) => {
        if (seq !== refreshSeqRef.current) return;
        setPsStatus(result);
        setUpiaFound(Boolean(result.upia_path));
        if (result.photoshop_versions.length === 0) {
          setStatus("error");
          setMessage("未检测到 Photoshop 2022+，请先安装 Photoshop");
        }
      })
      .catch((err) => {
        if (seq !== refreshSeqRef.current) return;
        setUpiaFound(false);
        setStatus("error");
        setMessage(`检测失败: ${err}`);
      })
      .finally(() => {
        if (seq !== refreshSeqRef.current) return;
        setRefreshing(false);
      });
  }, []);

  const installCcx = useCallback(
    (path: string) => {
      if (photoshopFound === false) {
        setStatus("error");
        setMessage("未检测到 Photoshop 2022+，请先安装 Photoshop");
        return;
      }

      setStatus("installing");
      setMessage("正在安装…");

      invoke<InstallResult>("install_ccx", { path })
        .then((result) => {
          setStatus("success");
          setMessage(result.message);
          refreshPsStatus();
        })
        .catch((err) => {
          setStatus("error");
          setMessage(`安装出错: ${err}`);
        });
    },
    [photoshopFound, refreshPsStatus]
  );

  const pickCcx = async () => {
    if (photoshopFound === false || isBusy) return;

    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "CCX 插件", extensions: ["ccx"] }],
    });

    if (typeof selected === "string") {
      installCcx(selected);
    }
  };

  const uninstallPlugin = async (plugin: UxpPlugin) => {
    if (isBusy) return;
    const confirmed = await ask(
      `确定卸载 ${plugin.name}？\n只会删除第三方 UXP 插件目录。`,
      { title: "卸载插件", kind: "warning" }
    );
    if (!confirmed) return;

    setStatus("installing");
    setMessage(`正在卸载 ${plugin.name}…`);

    try {
      const result = await invoke<InstallResult>("uninstall_uxp", {
        id: plugin.id,
        hostVersion: plugin.host_version,
        source: plugin.source,
      });
      setStatus("success");
      setMessage(result.message);
      refreshPsStatus();
    } catch (err) {
      setStatus("error");
      setMessage(`卸载出错: ${err}`);
    }
  };

  useEffect(() => {
    refreshPsStatus();
  }, [refreshPsStatus]);

  useEffect(() => {
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];
    let dragTimer: ReturnType<typeof setTimeout> | undefined;

    const armDragWatchdog = () => {
      if (dragTimer) clearTimeout(dragTimer);
      dragTimer = setTimeout(() => setDragging(false), DRAG_RESET_MS);
    };

    const subscribe = async () => {
      const handlers: Array<[string, Parameters<typeof listen>[1]]> = [
        [
          "tauri://drag-drop",
          (event) => {
            if (dragTimer) clearTimeout(dragTimer);
            setDragging(false);
            if (photoshopFoundRef.current === false || isBusyRef.current) return;

            const payload = event.payload as DragDropPayload;
            const paths = payload?.paths ?? [];
            const ccx = paths.find((p) => p.toLowerCase().endsWith(".ccx"));
            if (!ccx) {
              setStatus("error");
              setMessage("请拖入 .ccx 文件");
              return;
            }

            installCcx(ccx);
          },
        ],
        [
          "tauri://drag-enter",
          () => {
            setDragging(true);
            armDragWatchdog();
          },
        ],
        [
          "tauri://drag-over",
          () => {
            armDragWatchdog();
          },
        ],
        [
          "tauri://drag-leave",
          () => {
            if (dragTimer) clearTimeout(dragTimer);
            setDragging(false);
          },
        ],
      ];

      for (const [event, handler] of handlers) {
        const fn = await listen(event, handler);
        if (cancelled) {
          fn();
          return;
        }
        unlisteners.push(fn);
      }
    };

    void subscribe();

    return () => {
      cancelled = true;
      if (dragTimer) clearTimeout(dragTimer);
      for (const fn of unlisteners) fn();
    };
  }, [installCcx]);

  const zoneClass = [
    "drop-zone",
    dragging ? "drag-over" : "",
    status === "installing" ? "installing" : "",
    status === "success" ? "success" : "",
    status === "error" ? "error" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const latestPs = psStatus?.photoshop_versions[0];
  const externalPlugins = psStatus?.installed_uxp.filter((plugin) => plugin.source !== "Internal") ?? [];
  const thirdPartyPlugins = externalPlugins.filter((plugin) => !plugin.id.startsWith("com.adobe."));

  return (
    <div className="app">
      <h1 className="title">PS 增效工具安装器</h1>
      <p className="subtitle">支持 Photoshop 2022+ · 无需 Creative Cloud</p>

      <button className={zoneClass} type="button" onClick={pickCcx} disabled={photoshopFound === false || isBusy}>
        <div className="drop-icon">{isBusy ? <span className="progress-ring" /> : "📦"}</div>
        <div className="drop-text">
          {isBusy ? (
            <>
              正在安装…
              <br />
              <span className="drop-hint">请稍候，不要关闭窗口</span>
            </>
          ) : (
            <>
              将 <strong>.ccx</strong> 文件拖入此处
              <br />
              或点击选择插件文件
            </>
          )}
        </div>
      </button>

      {message ? <div className={`status-bar ${status}`}>{message}</div> : null}

      <section className="info-panel">
        <div className="info-row">
          <span className="info-label">当前 Photoshop</span>
          <span className="info-value">
            {latestPs ? `${latestPs.name} ${latestPs.version}` : "未检测到"}
          </span>
        </div>
        {psStatus && psStatus.photoshop_versions.length > 1 ? (
          <div className="muted-line">
            已安装 {psStatus.photoshop_versions.length} 个版本: {psStatus.photoshop_versions.map((ps) => ps.version).join(" / ")}
          </div>
        ) : null}

        <div className="plugin-header">
          <span>已安装 UXP</span>
          <div className="plugin-header-actions">
            <span>{thirdPartyPlugins.length} 个</span>
            <button className="refresh-button" type="button" onClick={refreshPsStatus} disabled={refreshing}>
              {refreshing ? "刷新中" : "刷新"}
            </button>
          </div>
        </div>
        <div className="plugin-list">
          {thirdPartyPlugins.length > 0 ? (
            thirdPartyPlugins.map((plugin) => (
              <div className="plugin-item" key={`${plugin.source}-${plugin.host_version}-${plugin.id}`} title={plugin.path}>
                <div className="plugin-main">
                  <span className="plugin-name">{plugin.name}</span>
                  <div className="plugin-actions">
                    <span className="plugin-version">{plugin.version}</span>
                    <button className="uninstall-button" type="button" onClick={() => uninstallPlugin(plugin)}>
                      卸载
                    </button>
                  </div>
                </div>
                <div className="plugin-meta">
                  {plugin.id} · PHSP {plugin.host_version} · {plugin.source}
                </div>
              </div>
            ))
          ) : (
            <div className="empty-list">未检测到第三方 UXP 插件</div>
          )}
        </div>
      </section>

      <div className="footer">
        {photoshopFound === true
          ? upiaFound
            ? "✓ 已检测到 Photoshop · Adobe 安装器可用"
            : "✓ 已检测到 Photoshop · 使用本地安装"
          : photoshopFound === false
          ? "✗ 未检测到 Photoshop"
          : "检测中…"}
      </div>
    </div>
  );
}
