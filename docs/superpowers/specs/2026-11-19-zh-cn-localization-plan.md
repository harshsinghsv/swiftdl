# 简体中文本地化：实现记录

设计见 [2026-11-19-zh-cn-localization-design.md](2026-11-19-zh-cn-localization-design.md)。
分支 `feat/zh-cn`。

## 语言行为

- 默认「跟随系统」：前端读 `navigator.languages`，`zh*` → `zh-CN`，其余 → `en`。
- 设置 → General → Language 可选「跟随系统 / English / 简体中文」，保存后立即生效，
  持久化在 `Settings.language`。
- 前端把解析后的语言通过 `set_ui_language` 告诉 Rust；托盘、通知、窗口标题、
  状态 notes 以 Rust 侧为准。
- Rust 在收到前端结果前用注册表（Windows）或 `LC_ALL`/`LANG`（Unix）猜一次。

## 前端

| 文件 | 作用 |
| --- | --- |
| `app/ui/src/locales/zh-CN.ts` | 中文词典；英文原文即 key，`as const` |
| `app/ui/src/i18n.ts` | `t`、`tt`、`tb`、`tn`、`translateDom`、`applyStatic`、`setLang`、`resolveLang` |
| `app/ui/src/main.ts` | 主窗口全部动态文案；语言选择器；`applyLanguage()` |
| `app/ui/src/download.ts` | 新建下载 / 进度窗口全部动态文案 |
| `app/ui/src/util.ts` | 时长、相对时间、日期格式化 |
| `app/ui/index.html` | 设置中的 Language 行 |
| `app/ui/src/api.ts` | `Settings.language` 与 `set_ui_language` 命令 |
| `app/ui/src/mock.ts` | 预览后端的语言字段；`?lang=zh-CN` 可指定语言 |

`translateDom` 自动翻译静态 HTML，跳过会由代码重绘的容器（`#rows`、`#view`、
`#toasts` 等）。key 拼错或漏翻会让 `npm run check` 失败。

## Rust 系统层

| 文件 | 改动 |
| --- | --- |
| `app/src-tauri/src/i18n.rs` | `Lang`、进程级 `AtomicU8`、`tr` / `note` / `peers` / `text`、系统语言检测 |
| `settings.rs` | `language: String`，默认 `"system"`，旧配置自动补默认值 |
| `manager.rs` | 启动时初始化语言；`set_settings` / `set_ui_language` 切换并重建托盘；通知、解压结果、关机提示、校验与换地址错误、状态 notes |
| `main.rs` | 托盘菜单走 `i18n::tr`；`rebuild_tray`；注册 `set_ui_language` |
| `commands.rs` | 新增命令；用户可见错误改走 `i18n::text` |
| `media.rs` / `stream.rs` / `torrent.rs` | 引擎状态 notes 与 torrent 用户/做种数 |
| `remote.html` | 内联中英字典 + 页头 EN/中文切换，选择存 `localStorage` |

## 浏览器扩展

- `manifest.json`：`default_locale: "en"`，名称/描述/按钮标题用 `__MSG_*`。
- `_locales/en|zh_CN/messages.json`：弹窗、浮动按钮、右键菜单、错误提示。
- `popup.js`：`data-i18n*` 标记与动态文案。
- `content.js`：按钮、菜单、状态、清晰度菜单；“Audio only”/“Original” 仅显示翻译，
  内部仍用英文比较。
- `background.js`：右键菜单标题。

扩展跟随浏览器界面语言，扩展内没有单独开关。

## CLI

`swiftdl --help` 与运行输出按 `SWIFDL_LANG`（优先）、`LC_ALL`、`LANG` 切换，
`zh*` 显示中文；默认英文。包括 `--help` 的结构性标题与内置 `-h`/`-V` 说明。

## 文档与站点

- `README.zh-CN.md`；英文 README 顶部有语言链接。
- `site/zh/{index,download,changelog}.html`，英文页面页头有「中文」入口。
- `site/site.js` 按 `<html lang>` 输出中文的平台说明、文件类型、演示动画文案与日期。
- `site/assets/app-dark-zh.png`：中文界面截图（`?lang=zh-CN` + 隐藏预览角标）。

## 验证

- `npm run check`、`npm run build`：通过。
- `cargo check --workspace --all-targets`：通过。
- `cargo test --workspace`：59 个测试通过（含 i18n 测试）。
- `cargo clippy --workspace --all-targets`：改动文件无新告警。
- `SWIFDL_LANG=zh_CN swiftdl --help`：全中文；默认英文。
- 无头 Edge 截图：主窗口、新建下载、设置（含 Language 行）、切回英文、站点中文页。

## 已知取舍

- 后端来自服务器、yt-dlp、librqbit 的原始错误文本保持英文（属于技术细节）。
- 已打开窗口的标题不追溯修改；新窗口使用当前语言。
- 应用内预览角标「Preview · simulated downloads」仅开发模式出现，未翻译。
- 扩展的站点商店文案（`docs/store/`）与 `CHANGELOG.md` 保持英文。
