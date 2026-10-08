# JIsjtu · 交我集

上海交通大学校园信息查询助手兼学习工作小助手。支持通过自然语言查询 Canvas 课程、作业及课件，读取校园邮件、阅读 PDF、打开水源社区与教务网，也可运行命令、处理文件和完成编码任务，支持url/名称唤起任意网站

现已支持多sessions热切换，skills读取，以及交大sso自动化登陆and持久化使用

建议使用deepseek相关模型以提升运行速度

前端使用 HTML / CSS / JavaScript，后端使用 Rust + Axum，通过 WebSocket 传递回答与工具进度。安装后的 `JIsjtu` 命令默认使用终端界面；`JIsjtu --web` 才打开网页，无需单独启动前端。

## 速度启动
下载JIsjtu.zip
解压后查看 README.md。

## 使用安装包

仓库中的 [JIsjtu.zip](JIsjtu.zip) 适用于 **Apple Silicon Mac（arm64）**。下载、解压，在解压目录运行：

```sh
sh install.sh
```

脚本不需要 sudo，会将程序和前端安装到 `~/.local/share/JIsjtu`，在 `~/.local/bin` 注册 `JIsjtu` 命令，并配置 zsh / bash 的 PATH。重复安装保留已有 `.env`。安装完成后可以删除解压目录。

首次安装后编辑配置文件（`.env` 是文件，不是目录）：

```sh
vim ~/.local/share/JIsjtu/.env
```

填写模型服务、Canvas、邮箱等配置，再新开终端运行：

```sh
JIsjtu
```

`JIsjtu` 默认启动 TUI，可输入 `/sessions` 切换会话。运行 `JIsjtu --web` 或 `JIsjtu web` 才会打开网页，默认地址为 `http://127.0.0.1:13376`。终端需保持运行，按 `Ctrl+C` 停止。`JIsjtu --config` 可查看配置路径，`JIsjtu --tui` 显式使用终端界面，`JIsjtu --sso-login` 单独检查登录。配置修改或升级后需重启。

## 配置

配置示例见 [agent_backend/.env.example](agent_backend/.env.example)。个人 `.env` 不提交到 Git，也不打进分发包。

| 变量 | 用途 |
| --- | --- |
| `OPENAI_BASE_URL` | OpenAI 兼容服务的接口地址； |
| `OPENAI_API_KEY` | 模型服务密钥 |
| `MODEL` | 模型名称 |
| `CANVAS_API_TOKEN` | 在 Canvas 的「账户 → 设置」创建访问令牌，用于课程、作业、课件查询 |
| `EMAIL_USER_ACCOUNT` / `EMAIL_USER_PASSWORD` | jAccount 账号或交大邮箱地址，以及 jAccount 登录密码 |
| `IMAP_PASSWORD` | 可选，独立的邮箱密码或客户端授权码；留空时沿用 `EMAIL_USER_PASSWORD` |
| `IMAP_HOST` / `IMAP_PORT` | 邮箱服务器；学校邮箱可设置 `imap.sjtu.edu.cn`、`993` |
| `AGENT_HTTP_PORT` / `AGENT_TCP_PORT` | HTTP 与 TCP 端口，默认 `13376` / `8081` |
| `AGENT_NO_BROWSER` | 设置此变量可跳过自动打开浏览器 |
| `AGENT_FRONTEND_DIR` | 可选，指定前端静态文件目录 |
| `AGENT_SKILLS_DIR` | 可选，指定技能目录；默认找当前目录或上一级的 `skills/` |

模型配置用于对话；Canvas 和邮箱配置按需填写。

## jAccount 自动登录

Agent 启动时自动运行 `sso_login`：从 `.env` 读取 `EMAIL_USER_ACCOUNT` 和 `EMAIL_USER_PASSWORD`，默认在后台无窗口的专用浏览器中打开 jAccount，用本地 `ddddocr` 识别图片验证码并填写。登录过程不弹出窗口、不抢占主窗口焦点。收到认证接口确认后保存登录状态、关闭登录页；已有有效会话时直接复用。交大邮箱地址会自动取 `@` 前的账号部分。

需要本机安装 Edge、Chrome 或 Chromium，以及 Python 3.10+；默认优先使用 Edge，未安装时再查找 Chrome / Chromium。在仓库根目录安装 Python 依赖：

```sh
python3 -m pip install -r agent_backend/requirements-sso.txt
```

安装包内附 `requirements-sso.txt`，安装后也会复制到应用目录。安装包使用者可按随包 README 创建 Python 虚拟环境，再安装此依赖清单。在 `.env` 中用 `AGENT_SSO_PYTHON` 指定该环境的 Python。`AGENT_SSO_BROWSER` 可指定浏览器可执行文件，优先于自动查找。

登录状态默认保存在当前工作目录的 `.sso/`，可用 `AGENT_SSO_DIR` 指定其他私有目录；不同账号使用各自的配置，同一账号切换浏览器或窗口模式时使用独立的浏览器配置并同步已保存的 Cookie。Cookie 不会返回给模型或写入聊天记录。校园查询复用已保存的登录状态，非校园网站使用独立的匿名浏览器配置。常用浏览器的其他配置不会自动获得该会话。Canvas 的现有 API 工具仍使用 `CANVAS_API_TOKEN`。

网页工具通过 `mode` 区分展示与查询：`show` 打开可见网页并保留给用户；`read` 在后台返回正文和链接，完成后自动关闭临时页面及其弹窗，失败、取消或助手进程断开时也会清理。后台登录和查询使用临时浏览器配置，任务结束后退出专用浏览器并删除临时配置，已保存的登录状态仍可复用；独立管理进程负责在助手意外退出时回收浏览器，无响应时只终止自己创建的后台进程。水源工具 `watch_shuiyuan` 默认 `show`，只有让助手查询、阅读或总结内容时才使用 `read`；`watch_eduinfo` 和 `open_usual_website` 默认 `read`。省略 `mode` 或传 `null` 使用对应工具的默认值，显式传值会覆盖默认值。查询课程、检索资料等中间操作应明确使用 `read`，在聊天中给出结果。自动清理仅作用于本次操作创建的后台浏览器与临时上下文，用户已有页面及明确展示的窗口不受影响。需要调试登录或查询时，可设置 `AGENT_SSO_HEADLESS=0` 并重启 Agent。

模型自行编写的脚本也应使用后台浏览器，并用 `try/finally` 关闭自己创建的临时页面、窗口和子进程；为分析文件时优先下载后读取，避免唤起桌面应用。

验证码错误最多尝试三次，账号密码错误立即停止。后台登录失败时关闭隐藏登录页、提示原因，Agent 继续启动；需要手动验证时，在 `.env` 设置 `AGENT_SSO_HEADLESS=0` 后重新启动登录，可见模式会保留失败的登录页供用户处理。手动完成验证后再次运行 `sso_login` 即可保存登录状态，再改回 `AGENT_SSO_HEADLESS=1` 恢复后台登录。`EMAIL_USER_PASSWORD` 必须是 jAccount 登录密码；邮箱客户端授权码请单独填入 `IMAP_PASSWORD`。设 `AGENT_SSO_AUTO_LOGIN=0` 可跳过启动检查，之后仍能调用无参数工具 `sso_login`。也可在 `agent_backend` 目录单独运行：

```sh
cargo run --locked -- --sso-login
```

SSO 测试使用本地模拟认证服务、临时浏览器配置及模拟账号，不访问真实 jAccount：

```sh
python3 -m unittest discover -s agent_backend/scripts -p test_sso.py -v
```

## 会话切换

左侧「我的对话」显示已保存的会话，点击即可切换，不需要刷新页面。「开启new session」会建立独立上下文，原会话仍保留。每个会话的输入草稿、滚动位置和工具进度独立；切走后任务继续执行，列表显示处理状态，切回时同步完整记录。草稿和滚动位置在当前页面内保留，聊天记录与模型上下文保存到磁盘，刷新或服务重启后仍可继续。

TUI 模式（`cargo run --locked -- --tui`）下，输入 `/sessions` 并按 Enter，进入独立的会话选择界面。用 `↑/↓` 选择，Enter 打开会话并返回聊天，Esc 取消返回，`r` 刷新列表。列表按最近更新时间排序，并标出当前会话和正在处理的会话；模型回复中也可以切换，切回后同步最新内容。每个会话的草稿和滚动位置在本次 TUI 运行期间独立保留，`/sessions` 不会发给模型或写入聊天历史。

会话由服务端管理，不依赖模型调用文件工具保存。`sessions/<稳定ID>.json` 保存完整模型消息（含工具调用及结果）、页面事件、创建时间和标题；标题取首次提问的前 28 个字符。`sessions/config.json` 是旧的格式示例，不作为聊天记录读取。可通过 `AGENT_SESSIONS_DIR` 指定存储位置，默认使用当前目录或上一级已有的 `sessions/`。

接口：`GET /api/sessions` 列表，`POST /api/sessions` 新建，`GET /api/sessions/{id}` 历史；WebSocket `/ws?session_id=<id>` 恢复会话，发送 `{"type":"switch_session","session_id":"<id>"}` 热切换。服务端返回 `session_snapshot` 或带会话 ID、递增 revision 的 `session_event`，前端确认切换后才允许发送新消息。

本次接口需要配套新版后端。前端测试使用本地模拟模型，不消耗真实 API 额度：安装 Playwright 后运行 `node agent_frontend/tests/sessions.cjs`；`PLAYWRIGHT_MODULE` 可指定已有 Playwright 模块路径，`AGENT_TEST_BINARY` 可指定已构建后端路径。

## 技能

「技能」是写给模型看的 Markdown 说明书，用来把某类任务的流程固定下来，比如校园卡查询步骤、某门课课件的下载方式、某个接口的参数和坑点。技能是纯文本，**新增后不需要重新编译**。

在 `skills/` 下新建目录并放一份 `SKILL.md`：

```
skills/
  campus-card/
    SKILL.md
```

```markdown
---
name: campus-card
description: 校园卡余额与消费记录查询流程
---

（正文：什么时候用、具体步骤、参数、注意事项）
```

后端启动时扫描技能目录，把 `name` 和 `description` 注入系统提示词；模型判断请求相符时，会先用 `read` 工具读取该文件全文，再按其中的步骤执行。技能正文只在需要时才进入上下文，所以可以写得很长。

启动日志会打印加载结果：

```
已从 skills 加载 2 个技能：campus-card、exam-query
```

技能只在启动时扫描，改完要重启服务。格式细节和目录查找顺序见 [skills/README.md](skills/README.md)。

## 从源码运行

需要支持项目所用 API 的近期稳定版 Rust（已在 Rust 1.97.1 验证），无需 Node。克隆项目后：

```sh
cd agent_backend
cp .env.example .env
# 编辑 .env，填写个人配置
cargo run --locked
```

保留同级 `agent_frontend` 目录。前端可直接编辑并刷新网页。

## 构建安装包

在 `agent_backend` 目录运行：

```sh
sh scripts/package.sh
```

输出 `agent_backend/dist/JIsjtu-<平台>.zip`，同时更新仓库根目录的 `JIsjtu.zip` 和 `install.sh`。安装包包含后端程序、前端、技能、安装说明、`.env.example`、SSO 依赖清单和许可证，不包含个人配置、Cookie 或会话。安装脚本统一由 `agent_backend/packaging/install.sh` 生成；根目录脚本也可以直接从仓库安装已有的 release。

已有经过验证的当前平台 release 时，可直接打包该文件：

```sh
AGENT_PACKAGE_BINARY="$PWD/target/release/agent" sh scripts/package.sh
```

默认按当前系统及架构构建；Intel Mac、Apple Silicon Mac、Linux 需要分别构建。Python 和浏览器需在目标机器安装，SSO 依赖按包内说明安装。Linux 包仍依赖相应系统动态库，当前安装脚本不支持 Windows。

安装与启动验证：

```sh
python3 scripts/test-package.py
python3 scripts/test-package.py --smoke
python3 scripts/test-package.py --archive ../JIsjtu.zip --smoke
```

`--smoke` 在临时目录安装并检查 HTTP、静态文件与 TCP 服务，不修改真实 shell 配置。安装脚本支持 `--prefix /绝对路径`、`--no-path`、`--profile /配置文件路径`。

## 使用边界

这是个人本地助手，工具可以执行命令、读写本机文件。当前服务监听所有网卡，尚无用户认证，不应将 HTTP / TCP 端口开放到公网。模型调用及校园功能依赖对应服务可用性。macOS 分发程序未经 Developer ID 签名或公证。

## 许可证

[MIT](LICENSE)，Copyright (c) 2026 XIAOMIQ772。
