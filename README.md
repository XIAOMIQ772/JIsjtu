# JIsjtu · 交我集

上海交通大学校园信息查询助手兼学习工作小助手。支持通过自然语言查询 Canvas 课程、作业及课件，读取校园邮件、阅读 PDF、打开水源社区与教务网，也可运行命令、处理文件和完成编码任务。

建议使用deepseek相关模型以提升运行速度

前端使用 HTML / CSS / JavaScript，后端使用 Rust + Axum，通过 WebSocket 传递回答与工具进度。后端启动后会自动打开网页，无需单独启动前端。

## 速度启动
下载JIsjtu.zip
解压后查看READNME.md

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

程序默认打开 `http://127.0.0.1:8080`。终端需保持运行，按 `Ctrl+C` 停止。`JIsjtu --config` 可查看配置路径。配置修改后需重启。

## 配置

配置示例见 [agent_backend/.env.example](agent_backend/.env.example)。个人 `.env` 不提交到 Git，也不打进分发包。

| 变量 | 用途 |
| --- | --- |
| `OPENAI_BASE_URL` | OpenAI 兼容服务的接口地址； |
| `OPENAI_API_KEY` | 模型服务密钥 |
| `MODEL` | 模型名称 |
| `CANVAS_API_TOKEN` | 在 Canvas 的「账户 → 设置」创建访问令牌，用于课程、作业、课件查询 |
| `EMAIL_USER_ACCOUNT` / `EMAIL_USER_PASSWORD` | 邮箱账号及 IMAP 所需密码或客户端授权码 |
| `IMAP_HOST` / `IMAP_PORT` | 邮箱服务器；学校邮箱可设置 `imap.sjtu.edu.cn`、`993` |
| `AGENT_HTTP_PORT` / `AGENT_TCP_PORT` | HTTP 与 TCP 端口，默认 `8080` / `8081` |
| `AGENT_NO_BROWSER` | 设置此变量可跳过自动打开浏览器 |
| `AGENT_FRONTEND_DIR` | 可选，指定前端静态文件目录 |
| `AGENT_SKILLS_DIR` | 可选，指定技能目录；默认找当前目录或上一级的 `skills/` |

模型配置用于对话；Canvas 和邮箱配置按需填写。

## 会话切换

左侧「我的对话」显示已保存的会话，点击即可切换，不需要刷新页面。「开启新对话」会建立独立上下文，原会话仍保留。每个会话的输入草稿、滚动位置和工具进度独立；切走后任务继续执行，列表显示处理状态，切回时同步完整记录。草稿和滚动位置在当前页面内保留，聊天记录与模型上下文保存到磁盘，刷新或服务重启后仍可继续。

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

输出 `agent_backend/dist/JIsjtu-<平台>.zip`，包含安装脚本、后端程序和前端目录。默认按当前系统及架构构建；Intel Mac、Apple Silicon Mac、Linux 需要分别构建。Linux 包仍依赖相应系统动态库，当前安装脚本不支持 Windows。

安装与启动验证：

```sh
python3 scripts/test-package.py
python3 scripts/test-package.py --smoke
```

`--smoke` 在临时目录安装并检查 HTTP、静态文件与 TCP 服务，不修改真实 shell 配置。安装脚本支持 `--prefix /绝对路径`、`--no-path`、`--profile /配置文件路径`。

## 使用边界

这是个人本地助手，工具可以执行命令、读写本机文件。当前服务监听所有网卡，尚无用户认证，不应将 HTTP / TCP 端口开放到公网。模型调用及校园功能依赖对应服务可用性。macOS 分发程序未经 Developer ID 签名或公证。

## 许可证

[MIT](LICENSE)，Copyright (c) 2026 XIAOMIQ772。
