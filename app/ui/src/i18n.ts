// Tiny gettext-style i18n. The English text is the key: t("Pause all") returns
// Chinese when the app is in Chinese and the same English text otherwise.
// `t` only accepts keys that exist in locales/zh-CN.ts, so a typo or a missed
// translation is a compile error under `npm run check`.

import zhCN from "./locales/zh-CN";

export type MsgKey = keyof typeof zhCN;
export type Lang = "en" | "zh-CN";
type Vars = Record<string, string | number>;

let lang: Lang = "en";

export const getLang = (): Lang => lang;

/** The saved setting, "system", or an explicit language tag. */
export function resolveLang(setting?: string | null): Lang {
  if (setting === "en" || setting === "zh-CN") return setting;
  const tags = navigator.languages?.length ? navigator.languages : [navigator.language || "en"];
  return tags.some((tag) => /^zh(-|$)/i.test(tag)) ? "zh-CN" : "en";
}

/** The locale to hand to Intl and toLocaleString. */
export const locale = (): string => lang;

export function setLang(next: Lang) {
  lang = next;
  document.documentElement.lang = next;
  applyStatic();
  translateDom();
}

export function t(key: MsgKey, vars?: Vars): string {
  let s: string = lang === "zh-CN" ? zhCN[key] : key;
  if (vars) for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(String(v));
  return s;
}

/** t() for a key that is only known at runtime (categories, statuses…). */
export const tt = (key: string, vars?: Vars): string => t(key as MsgKey, vars);

/** English text coming from the backend; translated when we know it. */
export function tb(text: string | null | undefined): string {
  if (!text) return "";
  return lang === "zh-CN" ? (zhCN as Record<string, string>)[text] ?? text : text;
}

/** Singular/plural: English picks between two keys, Chinese maps both to one. */
export function tn(one: MsgKey, many: MsgKey, n: number, vars?: Vars): string {
  return t(n === 1 ? one : many, { n, ...vars });
}

// The original text is remembered per element, so switching languages back
// and forth never re-translates an already-translated string.
const original = new WeakMap<Element, { text?: string; html?: string }>();

/** Translates the markup in index.html and download.html. */
export function applyStatic(root: ParentNode = document) {
  root.querySelectorAll<HTMLElement>("[data-i18n], [data-i18n-html]").forEach((el) => {
    let o = original.get(el);
    if (!o) {
      o = {};
      original.set(el, o);
    }
    if (el.hasAttribute("data-i18n")) {
      if (o.text == null) o.text = (el.textContent || "").trim();
      el.textContent = t(o.text as MsgKey);
    }
    if (el.hasAttribute("data-i18n-html")) {
      if (o.html == null) o.html = el.innerHTML.trim();
      el.innerHTML = t(o.html as MsgKey);
    }
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    el.title = t(el.getAttribute("data-i18n-title") as MsgKey);
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-placeholder]").forEach((el) => {
    el.setAttribute("placeholder", t(el.getAttribute("data-i18n-placeholder") as MsgKey));
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-aria-label]").forEach((el) => {
    el.setAttribute("aria-label", t(el.getAttribute("data-i18n-aria-label") as MsgKey));
  });
}

// ---------- automatic translation of the static markup ----------
// index.html and download.html are written in English. Walking their text
// nodes and attributes lets the English text itself be the key, so there is
// nothing to annotate. Dynamic containers are skipped: their code calls t().
const SKIP = [
  "script", "style",
  "#nav", "#rows", "#toasts", "#menu", "#inspector", "#view",
  "#pal-list", "#grab-list", "#pl-list", "#browser-list",
  "#ext-status", "#ext-browsers", "#ext-step1", "#ext-path",
  "#update", "#onboard", "#add-chips", "#add-state", "#grab-state",
  "#pl-state", "#pl-title", "#version", "#net-foot", "#net-speed",
  "#net-unit", "#h-name", "#h-count",
].join(",");

const skipped = (node: Node): boolean => {
  const el = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
  return !!el?.closest(SKIP);
};

const textOriginal = new WeakMap<Text, string>();
const attrOriginal = new WeakMap<Element, Map<string, string>>();

export function translateDom(root: ParentNode = document.body) {
  const dict = zhCN as Record<string, string>;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const node = n as Text;
    if (skipped(node)) continue;
    let orig = textOriginal.get(node);
    if (orig == null) {
      orig = node.nodeValue || "";
      textOriginal.set(node, orig);
    }
    const key = orig.replace(/\s+/g, " ").trim();
    if (!key) continue;
    const zh = dict[key];
    if (lang === "zh-CN" && zh) {
      const lead = /^\s*/.exec(orig)?.[0] || "";
      const trail = /\s*$/.exec(orig)?.[0] || "";
      node.nodeValue = lead + zh + trail;
    } else if (lang === "en" && node.nodeValue !== orig) {
      node.nodeValue = orig;
    }
  }

  const els = root instanceof Element ? [root, ...root.querySelectorAll("*")] : [...root.querySelectorAll("*")];
  for (const el of els) {
    if (skipped(el)) continue;
    let saved = attrOriginal.get(el);
    if (!saved) {
      saved = new Map();
      attrOriginal.set(el, saved);
    }
    for (const attr of ["title", "placeholder", "aria-label"]) {
      const now = el.getAttribute(attr);
      if (now == null) continue;
      if (!saved.has(attr)) saved.set(attr, now);
      const orig = saved.get(attr)!;
      const key = orig.replace(/\s+/g, " ").trim();
      const zh = dict[key];
      if (lang === "zh-CN" && zh) el.setAttribute(attr, zh);
      else if (lang === "en" && now !== orig) el.setAttribute(attr, orig);
    }
  }
}
