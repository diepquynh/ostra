import type { IDisposable, IFunctionIdentifier, ILinkHandler } from "@xterm/xterm";

/** The part of xterm's parser API the muting needs, so tests can pass a fake. */
export interface ParserHooks {
  registerCsiHandler(id: IFunctionIdentifier, cb: (params: (number | number[])[]) => boolean): IDisposable;
  registerOscHandler(ident: number, cb: (data: string) => boolean): IDisposable;
  registerDcsHandler(
    id: IFunctionIdentifier,
    cb: (data: string, params: (number | number[])[]) => boolean,
  ): IDisposable;
}

const always = () => true;
/** `CSI Ps t` values that ask for a report; the others move or resize the window and are ignored by xterm anyway. */
const WINDOW_REPORTS = new Set([11, 13, 14, 15, 16, 18, 19, 20, 21]);

/**
 * Swallow every terminal query before xterm.js answers it. The server answers queries itself, once, whether any
 * number of browsers or none are watching; a browser reply on top would reach the harness as typed input.
 */
export function muteQueryReplies(parser: ParserHooks): IDisposable {
  const subs: IDisposable[] = [
    parser.registerCsiHandler({ final: "n" }, always),
    parser.registerCsiHandler({ prefix: "?", final: "n" }, always),
    parser.registerCsiHandler({ final: "c" }, always),
    parser.registerCsiHandler({ prefix: ">", final: "c" }, always),
    parser.registerCsiHandler({ prefix: "=", final: "c" }, always),
    parser.registerCsiHandler({ intermediates: "$", final: "p" }, always),
    parser.registerCsiHandler({ prefix: "?", intermediates: "$", final: "p" }, always),
    parser.registerCsiHandler({ prefix: ">", final: "q" }, always),
    parser.registerCsiHandler({ prefix: "?", final: "u" }, always),
    parser.registerCsiHandler({ final: "t" }, (params) => WINDOW_REPORTS.has(Number(params[0]))),
    parser.registerDcsHandler({ intermediates: "$", final: "q" }, always),
    parser.registerDcsHandler({ intermediates: "+", final: "q" }, always),
    ...[4, 10, 11, 12, 17, 19].map((ident) =>
      parser.registerOscHandler(ident, (data) => data.split(";").includes("?")),
    ),
  ];
  return { dispose: () => subs.forEach((s) => s.dispose()) };
}

/** Where a link in harness output may lead. Output is untrusted, so only web pages, and only after the user sees the address. */
export function safeLinkTarget(uri: string): string | null {
  try {
    const u = new URL(uri);
    return u.protocol === "http:" || u.protocol === "https:" ? u.href : null;
  } catch {
    return null;
  }
}

export const linkHandler: ILinkHandler = {
  allowNonHttpProtocols: false,
  activate: (_event, uri) => {
    const href = safeLinkTarget(uri);
    if (href && window.confirm(`Open this link from the terminal?\n\n${href}`))
      window.open(href, "_blank", "noopener,noreferrer");
  },
};
