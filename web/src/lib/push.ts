import { api } from "../api";

function urlBase64ToUint8Array(base64: string): Uint8Array<ArrayBuffer> {
  const padding = "=".repeat((4 - (base64.length % 4)) % 4);
  const b64 = (base64 + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(b64);
  const out = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

function toBase64Url(buf: ArrayBuffer | null): string {
  if (!buf) return "";
  const bytes = new Uint8Array(buf);
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function pushSupported(): boolean {
  return "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
}

export async function registerServiceWorker(): Promise<ServiceWorkerRegistration | null> {
  if (!("serviceWorker" in navigator)) return null;
  try {
    return await navigator.serviceWorker.register("/sw.js");
  } catch {
    return null;
  }
}

export async function currentSubscription(): Promise<PushSubscription | null> {
  if (!pushSupported()) return null;
  const reg = await navigator.serviceWorker.getRegistration();
  return (await reg?.pushManager.getSubscription()) ?? null;
}

/** Ask for notification permission and register this browser with the server. */
export async function enablePush(): Promise<string | null> {
  if (!pushSupported()) return "This browser does not support push notifications.";
  const permission = await Notification.requestPermission();
  if (permission !== "granted")
    return "Notifications are blocked for this site. Allow them in the browser's site settings.";
  const info = await api.info();
  if (!info.vapid_public_key) return "The server has no push key configured.";
  const reg = (await registerServiceWorker()) ?? (await navigator.serviceWorker.ready);
  const sub =
    (await reg.pushManager.getSubscription()) ??
    (await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(info.vapid_public_key),
    }));
  await api.pushSubscribe({
    endpoint: sub.endpoint,
    keys: { p256dh: toBase64Url(sub.getKey("p256dh")), auth: toBase64Url(sub.getKey("auth")) },
  });
  return null;
}

export async function disablePush(): Promise<void> {
  const sub = await currentSubscription();
  await sub?.unsubscribe();
}
