# SwiftDL 简体中文本地化设计

日期：2026-11-19
状态：已确认

## 目标

为 SwiftDL 增加简体中文界面，默认跟随系统语言，用户可在设置中手动切换。
英文继续作为默认语言和回退语言。翻译文件独立，业务逻辑不变，便于同步上游。

## 范围

完整覆盖：

1. 桌面主窗口与每个下载窗口（`app/ui`）
2. Rust 系统层：托盘菜单、系统通知、窗口标题、后端状态提示与自有错误
3. 浏览器扩展（popup、content script、右键菜单）
4. Web 远程页面（`remote.html`）
5. CLI（帮助与输出）
6. README 与站点文案（第三阶段）

不翻译：文件名、URL、域名、yt-dlp/ffmpeg 原始报错、日志、HTTP 服务器原文。

## 语言解析

- `Settings` 新增 `language: "system" | "en" | "zh-CN"`，默认 `"system"`。
- `system`：按 `navigator.language` / `navigator.languages[0]` 判定，`zh*` 映射到 `zh-CN`，其余映射到 `en`。
- 设置 → General 顶部提供「跟随系统 / English / 简体中文」分段选择，保存后立即生效。
- 前端启动时把最终解析出的语言通过新命令告知 Rust；Rust 据此重建托盘、发送通知、设置新窗口标题。
- Rust 在收到前端语言前用系统注册表（Windows）/ 环境变量（Unix）做一次最佳猜测。

## 前端 i18n 架构

```
app/ui/src/i18n.ts           运行时：t、tt、tb、tn、translateDom、applyStatic、setLang、resolveLang
app/ui/src/locales/zh-CN.ts  中文字典，as const；英文原文本身是 key
```

- **英文原文即 key**（gettext 风格）：`t("All downloads")`。`t` 的参数类型是
  `keyof typeof zhCN`，所以拼错的 key 和漏翻的字符串都会让 `npm run check` 失败，
  同时不需要另建一份英文 key 表。
- 静态 HTML 不加任何标记：`translateDom` 遍历文本节点和 `title`/`placeholder`/
  `aria-label`，用 WeakMap 记住原文，切回英文时还原；会由代码重绘的动态容器
  （`#rows`、`#view` 等）跳过，避免与 `render()` 冲突。
- 动态字符串用 `t(key, vars)`；后端传来的英文原文用 `tb(text)` 翻译；
  复数用 `tn(one, many, n)`；运行时拼出的 key 用 `tt(key)`。
- `data-i18n*` 属性仍受支持（`applyStatic`），用于确实需要显式标注的少数位置。
- `document.documentElement.lang` 随语言更新。

## 翻译边界

- 分类显示名本地化（视频、音乐、程序……），但**内部标识和磁盘文件夹名保持英文**
  （Video、Music、Programs……），避免破坏已有下载和 Rust 的分类逻辑。
- 日期、相对时间用所选 locale 的 `Intl` 格式化；字节单位保持 B/KB/MB。
- 文件名、URL、技术报错保持原文。
- 界面字体栈追加 `"Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC"`。

## Rust 方案

新增 `app/src-tauri/src/i18n.rs`：

- `Lang { En, ZhCn }`，进程级 `AtomicU8`；`from_setting` / `from_code` / `system()`。
- `tr(&'static str)` 处理固定串（托盘菜单、通知标题、窗口标题），
  `note(&str)` 处理引擎状态 notes，`peers(n, seeds)` 处理带数字的 torrent 状态，
  `text(&str)` 处理返回给前端的短错误信息。
- 新增 `set_ui_language` 命令：前端解析出最终语言后调用；语言变化时
  `remove_tray_by_id("main")` + 重建托盘。
- 通知正文、解压结果、关机提示按当前语言拼接。
- 已打开的下载窗口标题不追溯修改，新窗口使用新语言。

## 扩展与远程

- 扩展：新增 `_locales/en/messages.json`、`_locales/zh_CN/messages.json`，
  `manifest.json` 加 `"default_locale": "en"`；JS 使用 `chrome.i18n.getMessage`，
  自动跟随浏览器界面语言。
- 远程页面：内联小字典，默认浏览器语言，提供 EN/中文切换并存 `localStorage`。

## CLI

- 固定输出、`about` 与参数帮助按 `SWIFDL_LANG`（优先）/ `LC_ALL` / `LANG` 本地化；
  参数名、错误详情保持英文。
- 使用独立的精简字典，不依赖桌面应用代码。

## 文档与站点

- `README.zh-CN.md`，英文 README 顶部加入链接。
- `site/zh/` 提供中文首页、下载页、更新日志，带语言切换；`site.js` 的动态文案
  （平台说明、文件类型、演示动画）按 `<html lang>` 切换。
- 中文首页使用单独截取的 `site/assets/app-dark-zh.png`；`CHANGELOG.md` 保持英文以方便上游协作。

## 分阶段

1. ✅ 桌面 UI + 系统层：i18n 核心、主窗口、下载窗口、语言设置、Rust 托盘/通知/标题
2. ✅ 扩展 + Web 远程 + CLI
3. ✅ README + 站点文案

实现状态与验证结果记录在 [2026-11-19-zh-cn-localization-plan.md](2026-11-19-zh-cn-localization-plan.md)。

## 验证

- `npm run check`、`npm run build`
- 浏览器中用 `mock.ts` 检查中英切换、明暗主题、主窗口与下载窗口
- `cargo test --workspace`、`cargo clippy --workspace`
- 扩展以 unpacked 方式加载后检查中文
- 上游贡献时补充中英文截图

## 风险

- 后端错误串数量多且分散；第一阶段只翻译自有、面向用户的错误，技术细节保留原文。
- 已打开窗口的标题不会随语言切换实时变化。
- 站点文案翻译属于纯文案工作，放在最后，不影响功能。
