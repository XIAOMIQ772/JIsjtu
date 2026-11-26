# JIsjtu · 交我集新手指南

交我集是上海交通大学学生的校园助手。仓库提供的 `JIsjtu.zip` 含有适用于 Apple M 系列芯片 Mac 的编译版。可以在苹果菜单的「关于本机」里查看芯片型号。

## 第一次使用

### 1. 双击安装

双击 `JIsjtu.zip` 解压，打开解压出的文件夹，双击 **Install.command**。终端显示「安装完成」后，首次安装会尝试用「文本编辑」打开你的个人配置文件 `.env`。

也可以在解压目录的终端运行 `sh install.sh`。要手动打开或再次编辑配置，在 Mac 上按 ⌘ 空格搜索「终端」，粘贴下面的命令并按回车。

```sh
open -e ~/.local/share/JIsjtu/.env
```

如果 Mac 提示“无法验证开发者”，先确认 ZIP 来自你信任的发布者，再按提示处理这次打开请求。[Apple 关于打开下载软件的说明](https://support.apple.com/zh-cn/102445)

### 2. 填写模型服务

在文件开头找到以下三项，把等号右侧换成你自己的值。API 密钥在模型服务商的控制台创建，模型 ID 从同一服务商对你的账号开放的模型列表中复制。模型需支持流式 Chat Completions 和工具调用。

```dotenv
OPENAI_API_KEY=填入你的API密钥
OPENAI_BASE_URL=https://api.openai.com/v1
MODEL=填入可用的模型ID
```

上面的基础地址以 OpenAI 为例。使用其他兼容服务时，填写该服务商提供的 API 基础地址，程序会自行拼接 `/chat/completions`。按 **⌘S** 保存。只想先体验对话时，校园账号可以留空。

### 3. 启动并提问

新开一个终端窗口，输入下面的命令并按回车。

```sh
JIsjtu
```

看到输入框后，试着问“现在北京时间几点？”并按回车。输入 `/resume` 可以打开历史对话。使用期间保持终端开启，按 `Ctrl+C` 退出。以后修改配置，退出后再启动即可生效。

要让新聊天记住答复语言、详略或排版，在个人 `.env` 中加入 `AGENT_DREAMING=1` 并重启。例如，“以后请用中文回答”会成为默认偏好。终端和网页都可以输入 `/memory` 查看，`/forget` 清空。

如果终端提示 `command not found`，可直接运行 `~/.local/bin/JIsjtu`。想用浏览器聊天，运行 `JIsjtu --web`，默认地址是 `http://127.0.0.1:13376`。用 `JIsjtu --doctor` 可检查模型接口、Canvas 令牌和邮箱等配置。

## 按需连接校园服务

继续编辑同一份 `.env`，按要使用的功能填写。

| 想做什么 | 填哪些配置 |
| --- | --- |
| 查 Canvas 作业、读课件、提交作业 | 在交大 Canvas 的「账户 → 设置」创建访问令牌，填入 `CANVAS_API_TOKEN`。可以问“这周还有哪些作业没交？” |
| 查老师评分、作业量、往年是否点名 | 在 `JCOURSE_EMAIL` 填选课社区的账号前缀或完整的 `@sjtu.edu.cn` 邮箱，在 `JCOURSE_PASSWORD` 填该社区的独立密码。可以问“数据结构哪位老师作业少？给我看看点评。” |
| 查教务课表、考试或水源帖子 | `EMAIL_USER_ACCOUNT` 填 jAccount 账号或交大邮箱，`EMAIL_USER_PASSWORD` 填 jAccount 登录密码，再完成下方的浏览器设置。 |
| 收发交大邮件 | `EMAIL_USER_ACCOUNT` 填交大邮箱。收件填 `IMAP_HOST=mail.sjtu.edu.cn`、`IMAP_PORT=993`，独立邮箱密码或客户端授权码填 `IMAP_PASSWORD`。发件默认使用 `mail.sjtu.edu.cn:465`，单独的发件授权码填 `SMTP_PASSWORD`。[学校邮箱服务器参数](https://net.sjtu.edu.cn/info/1191/2699.htm) |

收件密码留空时沿用 `EMAIL_USER_PASSWORD`，发件密码留空时依次使用 `IMAP_PASSWORD`、`EMAIL_USER_PASSWORD`。让助手代发邮件或水源帖子时，核对预览后，在同一对话输入预览里的确认口令。要审阅 Canvas 作业再交，可以先看提交预览，在同一对话 30 分钟内回复“提交”。

### 教务与水源的浏览器设置

准备 Edge、Chrome 或 Chromium 和 Python 3.10 或更高版本。在终端运行以下两条命令，为交我集安装浏览器连接和验证码识别依赖。

```sh
python3 -m venv ~/.local/share/JIsjtu/.venv
~/.local/share/JIsjtu/.venv/bin/python -m pip install -r ~/.local/share/JIsjtu/requirements-sso.txt
```

在个人 `.env` 另起一行填写以下设置，保存后运行 `JIsjtu --sso-login` 检查 jAccount 登录。

```dotenv
AGENT_SSO_PYTHON=./.venv/bin/python
```

启动器从安装目录运行，相对路径会指向刚创建的环境。程序默认在后台登录，再次查询时复用已保存的会话。要跳过启动时的 jAccount 检查，可设 `AGENT_SSO_AUTO_LOGIN=0`。遇到需要手动验证的登录页，可暂设 `AGENT_SSO_HEADLESS=0`，重新运行 `JIsjtu --sso-login`。

### 上课、作业和签到提醒

运行期间，macOS 可以在上课前 10 分钟、未完成作业截止前 1 小时发出通知。课表提醒使用已登录的教务会话，作业提醒使用 Canvas 令牌。设置 `AGENT_REMINDERS=0` 可以关闭这两类提醒。

要监看一节课的实时签到，可以说“帮我盯明天 8 点的大学物理有没有发起签到”。这需要 Python 3、Canvas 令牌，且课程菜单已开放「点名签到」。从课前 10 分钟至上课后 5 分钟，保持 Mac 唤醒并运行交我集，助手每 30 秒检查一次，有新记录就通知。

## 会话、安装位置和升级

- 在 `/resume` 列表中用 ↑ / ↓ 选择会话，回车打开。删除时按 `d` 或 Delete，核对标题后按 `y` 确认，按 `n` 或 Esc 取消。
- 程序和这份指南默认安装在 `~/.local/share/JIsjtu`。用 `JIsjtu --config` 查看个人配置文件的位置。自定义技能可放在安装目录的 `skills/<名字>/SKILL.md`，重启后加载。
- 升级时先退出交我集，解压新版 ZIP，再次双击 `Install.command` 或在解压目录运行 `sh install.sh`。安装器保留个人配置、历史会话、登录状态和自行添加的技能。新版本增加的配置项可在安装目录的 `.env.example` 中查看。安装完成后可以删除 ZIP 和解压文件夹。
- 使用自定义安装位置时，从解压目录运行 `sh install.sh --prefix /绝对路径`。`--no-path` 跳过写入 shell 配置，`--profile /文件路径` 指定写入位置。HTTP 和 TCP 默认监听本机的 `127.0.0.1`。
- 使用单独构建的 Linux 包时，在解压目录运行 `sh install.sh`，然后用文本编辑器打开 `~/.local/share/JIsjtu/.env`，从本指南第 2 步继续。

更多校园功能和配置选项见[项目首页](https://github.com/XIAOMIQ772/JIsjtu)。
