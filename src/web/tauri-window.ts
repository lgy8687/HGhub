/**
 * 网页版 shim：桌面窗口 API 在浏览器里不存在。
 * 返回万用空对象，所有方法都返回已 resolve 的 Promise，
 * 保证 `.then()` / `.catch()` 调用链不会抛错。
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function getCurrentWindow(): any {
  return new Proxy(
    {},
    {
      get: (_target, prop) => {
        if (prop === "label") return "web";
        if (prop === Symbol.toPrimitive || prop === "toString") return () => "[web window]";
        return (..._args: unknown[]) => Promise.resolve(undefined);
      },
    },
  );
}

export const getCurrentWebviewWindow = getCurrentWindow;
export const LogicalSize = class {
  constructor(public width: number, public height: number) {}
};
export const PhysicalSize = LogicalSize;
export const currentMonitor = async () => null;
export const primaryMonitor = async () => null;
