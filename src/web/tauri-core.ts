/**
 * 网页版 shim：把 Tauri 的 invoke 换成 HTTP 调用。
 * 由 vite.web.config.ts 的 alias 生效；原生三端构建完全不加载本文件。
 */
export async function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const res = await fetch("/api/cmd", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ cmd, args: args ?? {} }),
  });

  if (res.status === 401 || (res.redirected && res.url.includes("/login"))) {
    location.replace("/login");
    throw new Error("请先登录");
  }

  const text = await res.text();
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    throw new Error(`HTTP ${res.status}: 响应不是 JSON`);
  }
  if (!res.ok) {
    const msg = (data as { error?: string } | null)?.error;
    throw new Error(msg || `HTTP ${res.status}`);
  }
  return data as T;
}

/** 桌面端插件事件在浏览器里不存在，返回可安全解构的空监听。 */
export async function addPluginListener(
  ..._args: unknown[]
): Promise<{ unlisten: () => void }> {
  return { unlisten: () => {} };
}

/** 其余 core 导出（若未来引用）统一给可调用的空实现。 */
export const convertFileSrc = (src: string) => src;
export const isTauri = () => false;
