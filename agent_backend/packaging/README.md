# JIsjtu · 安装与升级

本包包含已编译的程序，无需安装 Rust 或 Node.js。安装脚本会检查系统和处理器架构是否匹配；仓库首页的 `JIsjtu.zip` 适用于 Apple Silicon Mac。

## 安装

在解压后的目录打开终端，运行：

```sh
sh install.sh
```

无需 sudo。程序安装到 `~/.local/share/JIsjtu`，命令安装到 `~/.local/bin/JIsjtu`。脚本会为 zsh / bash 注册 PATH；安装后新开一个终端即可使用。

编辑配置文件：

```sh
vim ~/.local/share/JIsjtu/.env
```

对话功能必填 `OPENAI_API_KEY`、`OPENAI_BASE_URL`、`MODEL`。按需填写 `CANVAS_API_TOKEN`、jAccount 账号密码和邮箱配置。配置示例随包提供为 `.env.example`，安装器在首次安装时复制为个人 `.env`。

## 浏览器与 jAccount 登录

网页操作需要 Python 3.10+ 和 Edge、Chrome 或 Chromium，程序优先使用 Edge。jAccount 自动登录还需要本地 OCR 依赖。

可以创建独立的 Python 环境并安装依赖：

```sh
python3 -m venv ~/.local/share/JIsjtu/.venv
~/.local/share/JIsjtu/.venv/bin/python -m pip install -r ~/.local/share/JIsjtu/requirements-sso.txt
```

在安装目录的 `.env` 中添加：

```dotenv
AGENT_SSO_PYTHON=./.venv/bin/python
```

启动器会先进入安装目录，所以这个相对路径可以直接使用。已有可用的 Python 环境时，也可将 `AGENT_SSO_PYTHON` 设为其解释器的绝对路径。依赖安装需要联网，浏览器和 Python 本身不包含在 ZIP 中。

`EMAIL_USER_ACCOUNT` 填 jAccount 账号或交大邮箱，`EMAIL_USER_PASSWORD` 填 jAccount 登录密码；邮箱客户端授权码单独填 `IMAP_PASSWORD`。

程序默认在后台登录和查询网页，任务结束后回收临时浏览器，保存的登录状态仍可复用。`AGENT_SSO_AUTO_LOGIN=0` 可跳过启动登录，`AGENT_SSO_HEADLESS=0` 可显示登录过程供手动验证或调试。

## 使用

```sh
JIsjtu              # 默认启动终端界面，输入 /sessions 切换会话
JIsjtu --web        # 启动网页界面，默认地址 http://127.0.0.1:13376
JIsjtu web          # 与 --web 相同
JIsjtu --tui        # 显式启动终端界面
JIsjtu --sso-login  # 单独检查或登录 jAccount
JIsjtu --config     # 显示个人配置文件位置
JIsjtu --help       # 查看帮助
```

运行时保持终端打开，按 Ctrl+C 停止服务。修改 `.env` 后重启生效。技能可以放入安装目录的 `skills/<名字>/SKILL.md`，新增后重启加载。

## 升级

退出正在运行的 JIsjtu，解压新版 ZIP，再次运行 `sh install.sh`。升级会替换程序、前端和随包技能，并保留个人 `.env`、`sessions/`、`.sso/` 和自行添加的技能。新增配置项可参考安装目录中更新后的 `.env.example`。升级后重新启动 JIsjtu。

安装完成后可以删除解压目录和 ZIP。自定义安装位置可运行 `sh install.sh --prefix /绝对路径`；`--no-path` 跳过修改 shell 配置，`--profile /文件路径` 指定 PATH 配置文件。
