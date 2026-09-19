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

模型配置用于对话；Canvas 和邮箱配置按需填写。

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
