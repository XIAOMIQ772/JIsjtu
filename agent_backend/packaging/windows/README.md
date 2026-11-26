# 交我集 Windows 版

适用于 Intel 或 AMD 处理器的 Windows 10 / 11 x64。安装在当前用户目录，终端版和网页版都已编译好。

## 安装与第一次对话

1. 右键 `JIsjtu-Windows-x64.zip`，选择「全部提取」。在解压后的文件夹里双击 **Install.cmd**。窗口显示「安装完成」时，首次安装会用记事本打开个人配置。
2. 在配置中填写模型服务商提供的 `OPENAI_API_KEY`、`OPENAI_BASE_URL` 和 `MODEL`。基础地址例如 `https://api.openai.com/v1`。模型应支持流式 Chat Completions 和工具调用。按 Ctrl+S 保存，再关闭记事本。
3. 在开始菜单打开 **JIsjtu**，输入「现在北京时间几点？」并回车。想在浏览器聊天，打开 **JIsjtu 网页版**。保持启动窗口开启，按 Ctrl+C 退出。网页默认地址为 `http://127.0.0.1:13376`。

开始菜单的 **JIsjtu 配置** 可以再次打开配置。修改后请重新启动程序。在新开的 PowerShell 或命令提示符中也可以使用以下命令。

| 命令 | 用途 |
| --- | --- |
| `JIsjtu` | 启动终端界面，在输入框中键入 `/resume` 可选择历史对话 |
| `JIsjtu --web` | 启动网页界面 |
| `JIsjtu --doctor` | 检查模型、令牌和账号配置 |
| `JIsjtu --config` | 显示个人配置文件的位置 |
| `JIsjtu --sso-login` | 检查或登录 jAccount |

要让新聊天记住答复语言、详略或排版，在个人配置中加入 `AGENT_DREAMING=1` 并重启。例如，“以后请用中文回答”会成为默认偏好。终端和网页都可以输入 `/memory` 查看，`/forget` 清空。

## 连接校园服务

| 功能 | 准备 |
| --- | --- |
| Canvas 作业、课件和提交 | 在 Canvas 的「账户 → 设置」创建访问令牌，填入个人配置的 `CANVAS_API_TOKEN`。 |
| 教务、课表或水源 | 填写 `EMAIL_USER_ACCOUNT` 和 `EMAIL_USER_PASSWORD`，按下文设置浏览器和 Python。 |
| 邮箱 | `EMAIL_USER_ACCOUNT` 填交大邮箱。收件设置 `IMAP_HOST=mail.sjtu.edu.cn`、`IMAP_PORT=993` 和 `IMAP_PASSWORD`，单独的发件授权码填入 `SMTP_PASSWORD`。 |
| 选课社区 | 填写选课社区的 `JCOURSE_EMAIL` 和 `JCOURSE_PASSWORD`。 |

使用浏览器登录或签到监看时，在开始菜单打开 **JIsjtu 校园依赖**。它联网下载依赖并安装在交我集专用的 `.venv` 中。它优先使用已有的 64 位 Python 3.10–3.12，缺少时尝试通过 WinGet 安装当前用户版 Python 3.12。若电脑上没有 WinGet，请先安装带 Python 启动器的 64 位 Python 3.12，再运行一次。验证码识别包的 Windows 支持范围见[项目说明](https://pypi.org/project/ddddocr/)。

准备好 Edge 或 Chrome 后，在新终端运行 `JIsjtu --sso-login`。需要手动验证时，在个人配置中设 `AGENT_SSO_HEADLESS=0` 后重试。已有其他 Python 环境时，可将 `AGENT_SSO_PYTHON` 指向安装了 `requirements-sso.txt` 中依赖的解释器。配置里的 Windows 路径可以写成 `C:/目录/python.exe`。

上课、作业截止提醒和签到监看使用 Windows 通知。在系统「设置 → 系统 → 通知」中允许 JIsjtu 发送通知，保持电脑唤醒、交我集运行。签到监看还需要有效的 Canvas 令牌及已开放「点名签到」的课程。

终端命令工具在 Windows 上执行 PowerShell 命令。安装包自带 PDFium、Tesseract 和简体中文、英文识别数据，可在本机离线渲染 PDF 原页、识别扫描件与图片。相关许可证随程序保存在 `pdf-tools` 中。

## 升级与安装位置

程序默认安装在 `%LOCALAPPDATA%\JIsjtu`。个人配置是该目录下的 `.env`，会话保存在 `sessions` 中。可把自定义技能放入 `skills\名称\SKILL.md` 并重新启动。

升级时先退出交我集，再解压新版并双击 **Install.cmd**。安装器会更新程序、前端和内置技能，保留个人配置、历史会话、登录状态、自建技能及已安装的 Python 环境。安装完成后可删除下载的 ZIP 和解压文件夹。

要自定义安装位置，可在解压目录的 PowerShell 中运行 `.\Install.cmd -InstallDir "D:\我的应用\JIsjtu"`。`-NoPath` 跳过注册终端命令，`-NoShortcuts` 跳过创建开始菜单入口，`-NoOpenConfig` 跳过打开记事本。使用系统通知时请保留开始菜单入口。
