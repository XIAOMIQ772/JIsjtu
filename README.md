# JIsjtu · 交我集

交我集是给上海交通大学学生用的校园助手。可以用自然语言查课表和作业截止时间、比较选课社区里的教师评价、读课件、写作业并提交到 Canvas。也可以查考试和成绩、估算 GPA、找空教室、读校园邮件和水源讨论，或让助手处理本地文件、运行命令、打开指定网址。

运行 `JIsjtu` 或 `JIsjtu --tui` 进入终端界面（TUI），运行 `JIsjtu --web` 或 `JIsjtu web` 打开网页。两个界面共用 Rust 后端，通过 WebSocket 接收回答和工具进度，会话保存在本机。

以往的版本记录见 [CHANGELOG.md](CHANGELOG.md)。

## 快速开始

### Windows 10 / 11 x64（Intel、AMD）

下载 [JIsjtu-Windows-x64.zip](JIsjtu-Windows-x64.zip)，右键选择「全部提取」，打开解压目录并双击 **Install.cmd**。程序装在当前用户的 `%LOCALAPPDATA%\JIsjtu`。首次安装会打开记事本，填写下方的三个模型配置项并保存。从开始菜单打开 **JIsjtu** 进入终端界面，或打开 **JIsjtu 网页版**。开始菜单里的 **JIsjtu 配置** 可随时编辑配置。[Windows 随包指南](agent_backend/packaging/windows/README.md)

### Apple Silicon Mac（M 系列）

下载 [JIsjtu.zip](JIsjtu.zip)。解压后双击 `Install.command`，安装器会把程序装到 `~/.local/share/JIsjtu`，并在 `~/.local/bin` 注册 `JIsjtu` 命令。也可在解压目录的终端运行 `sh install.sh`。

Mac 安装器会在首次安装后尝试用「文本编辑」打开个人配置文件。也可以随时在终端运行下面的命令打开它。

```sh
open -e ~/.local/share/JIsjtu/.env
```

### 首次对话

对话需要填写 `OPENAI_BASE_URL`、`OPENAI_API_KEY` 和 `MODEL`。模型服务需支持流式 Chat Completions 和工具调用。校园账号按要用的功能填写，保存后新开一个终端启动助手。

```sh
JIsjtu
```

在终端输入 `/` 可打开命令面板。`/resume` 打开历史会话，`/today` 看今日简报，`/ddl` 汇总作业截止，`/week` 看本周课表。网页模式的默认地址是 `http://127.0.0.1:13376`。保持终端运行，按 `Ctrl+C` 退出。用 `JIsjtu --config` 查看配置文件路径，修改配置或升级后重启生效。

包内 `README.md` 附有从安装到第一次提问的教程，也说明了怎样接入 Canvas、教务和选课社区。升级时重新运行安装器即可，个人配置和历史会话会保留。[Windows 指南](agent_backend/packaging/windows/README.md) · [Mac 指南](agent_backend/packaging/README.md)

## 配置

配置示例见 [agent_backend/.env.example](agent_backend/.env.example)。个人 `.env` 不提交到 Git，也不打进分发包。

| 变量 | 用途 |
| --- | --- |
| `OPENAI_BASE_URL` | 模型服务的 API 基础地址，例如 `https://api.openai.com/v1`。程序会自行拼接 `/chat/completions` |
| `OPENAI_API_KEY` | 模型服务密钥 |
| `MODEL` | 模型名称 |
| `CANVAS_API_TOKEN` | 在 Canvas 的「账户 → 设置」创建访问令牌，用于课程、作业、课件和课堂签到记录查询 |
| `EMAIL_USER_ACCOUNT` / `EMAIL_USER_PASSWORD` | jAccount 账号或交大邮箱地址，以及 jAccount 登录密码 |
| `JCOURSE_EMAIL` / `JCOURSE_PASSWORD` | 查选课评价时填写选课社区账号（邮箱前缀或完整的 `@sjtu.edu.cn` 邮箱）和独立的社区密码 |
| `IMAP_PASSWORD` | 可选，独立的邮箱密码或客户端授权码；留空时沿用 `EMAIL_USER_PASSWORD` |
| `IMAP_HOST` / `IMAP_PORT` | 收取交大邮件时将主机填为 `mail.sjtu.edu.cn`，加密端口默认为 `993`。[交大邮箱服务器信息](https://net.sjtu.edu.cn/info/1191/2699.htm) |
| `SMTP_HOST` / `SMTP_PORT` / `SMTP_PASSWORD` | 发信默认使用 `mail.sjtu.edu.cn:465`（TLS）。未单设密码时依次使用 `IMAP_PASSWORD`、`EMAIL_USER_PASSWORD` |
| `AGENT_BIND_ADDR` | HTTP 和 TCP 共用的监听 IP，默认 `127.0.0.1`。需要局域网访问时可指定本机的局域网 IP |
| `AGENT_HTTP_PORT` / `AGENT_TCP_PORT` | HTTP 与 TCP 端口，默认 `13376` / `8081` |
| `AGENT_REMINDERS` | Windows 和 macOS 运行期间提醒上课与作业截止，设为 `0` 可关闭 |
| `AGENT_NO_BROWSER` | 设置后，Web 模式启动时跳过自动打开聊天页面，可手动访问终端打印的地址 |
| `AGENT_FRONTEND_DIR` | 可选，指定前端静态文件目录 |
| `AGENT_SKILLS_DIR` | 可选，指定技能目录；默认找当前目录或上一级的 `skills/` |
| `SJTU_TERM_START` / `SJTU_TERM_WEEKS` / `SJTU_TERM_NAME` | 可按教务处校历填写第 1 周周一（YYYY-MM-DD）、总周数（默认 18）和学期名称 |

## 校园功能

| 想做的事 | 工具 | 数据来源 |
| --- | --- | --- |
| 今天有什么安排 | `daily_brief`：今天（晚上附明天）的课、14 天内考试、48 小时内截止和逾期作业、24 小时内公告、未读邮件，并发获取 | 教务系统、Canvas、邮箱 |
| 课表、下节课在哪 | `class_schedule`：今天/明天/本周/下周/整学期，按教学周与单双周过滤 | 教学信息服务网 |
| 考试时间与考场 | `exam_schedule`：倒计时、考场、未公布标注 | 教学信息服务网 |
| 成绩、GPA 和排名 | `score_report` 根据本人成绩估算总 GPA、学期 GPA 和加权均分。`view=official` 提供官方本人查询入口及学院排名证明办理方式 | 教学信息服务网、学院办事指南 |
| 离毕业还差什么 | `degree_progress`：培养方案各类要求的要求/已获得学分与缺口 | 教学信息服务网 |
| 我有哪些课 | `get_courses` 列出进行中的 Canvas 课程和课程 ID，并显示已有的课号、班号、学期和授课教师 | Canvas |
| 作业截止 | `get_deadlines`（跨课程）、`get_exam`（单门课，含提交状态） | Canvas |
| 课程公告 | `get_announcements` | Canvas |
| 平时成绩与评语 | `get_grades`：逐项得分、班级均分/最高/最低、权重、老师评语 | Canvas |
| 找空教室 | `find_classroom` 按教学周和节次查空闲教室，也能查指定教室的排课。查当天且传感器有数据时会显示实时人数 | 教室信息服务（公开） |
| 图书馆人多吗 | `library_status`：各馆在馆人数与占比 | 图书馆人数统计（公开） |
| 选哪位老师、往年点名吗 | `course_reviews` 按课程比较老师，或按老师分课程查均分、点评数、给分和工作量。点名体验附反馈学期和原文，读到的点评未提及时会标明未知。合授条目的评分属于教师组 | [交大选课社区](https://course.sjtu.plus/) |
| 水源 | `get_hotnews` 热帖简报、`shuiyuan_search` 搜索、`shuiyuan_topic` 读帖子、`shuiyuan_post` 查看版块、发表主题或回帖 | 水源社区 |
| 邮件 | `mail_fetch` 列出邮件、按发件人或主题搜索、阅读正文和下载附件，`send_email` 发送纯文本邮件 | 学校邮箱 |
| 找课件、下载整门课 | `search_files` 跨课程搜索，`course_files` 的 `download_all` 下载指定课程中可访问的全部文件，默认目录为 `~/Downloads/Canvas-<课程id>`。文件名带 Canvas ID，重跑会跳过已有且大小匹配的文件 | Canvas |
| 读课件、文档 | `pdf_analysis` 可提取 PDF 文本。在 Windows 和 macOS 上还支持离线识别扫描页和页内截图，或用 `image_page` 将指定页发送给支持图片输入的模型核对公式和图表。`read_office` 可读 `.docx` 的表格和公式、`.pptx` 的讲者备注和 `.xlsx` 的工作表 | 本地文件 |
| 写作业、交作业 | `homework` 读取题目、评分细则和附件，检查答案和必交文件后，可提交在线文本或上传文件。聊天中可先看预览，在同一会话 30 分钟内回复“提交”，也可事先说“写完直接交”。核实 Canvas 回执后会显示提交时间和次数 | Canvas、本地文件 |
| 上课、作业主动提醒 | 程序运行期间，Windows 和 macOS 在上课前 10 分钟、未完成作业截止前 1 小时发送通知。课表需要已登录的教务会话，作业需要 Canvas 令牌 | 教务系统、Canvas |
| 这节课发起签到了吗 | 对助手说“帮我盯明早 8 点的大学物理有没有发起签到”。`watch_attendance` 发现这节课时段内的新签到记录就发系统通知，并附上页面可见的本人签到状态 | Canvas 课堂签到 |
| 导入手机日历 | `export_calendar` 把按周展开的课表、已公布的考试和未来 60 天未完成的作业截止导出为 `.ics`，默认保存在 `~/Downloads`。考试和作业截止附带提醒 | 教务系统、Canvas |

签到监看需要 Windows 或 macOS、Python 3、有效的 Canvas 令牌，且该课的 Canvas 菜单已开放「点名签到」。给出课程、日期和开始时间就能安排监看，时间不全时会用教务课表定位。例如 08:00 的课会在 07:50 至 08:05 每 30 秒查一次，中途提出请求就从当时开始。请保持电脑唤醒并运行 JIsjtu。每条新记录只提醒一次。如果整个时段都读不到签到页面，结束时会通知查询失败。重启后会恢复尚未过期的任务。

课表、考试、作业截止和签到监看均按北京时间。内置作息第 1 节从 08:00 开始，每节 45 分钟。内置的 2026–2027 学年秋季学期从 2026 年 9 月 14 日开始，共 18 周，第 17–18 周为考试周。要使用尚未内置的学期，可按教务处公布的日期在 `.env` 填写 `SJTU_TERM_START`（第 1 周周一）和 `SJTU_TERM_WEEKS`。[教务处历年校历](https://www.jwc.sjtu.edu.cn/jxxl/lnxl.htm)

教务系统和水源需要 jAccount 登录态：浏览器助手先在后台打开站点入口页走完单点登录，再在同一页面里请求查询接口，多项查询共用一次浏览器会话；jAccount 会话过期时会自动重新登录一次。

同一轮中，如果全部调用都是支持并行的只读查询，程序会并发执行并按调用顺序返回结果。一轮里包含命令、写文件或提交作业时，整轮依次执行。

代发邮件或水源帖子时，先核对会话中显示的收件人、版块或主题和完整正文，然后在同一会话回复预览给出的 `确认发送 <口令>`。口令 15 分钟内有效，每个会话只保留最新一份待发草稿。

安装版可用下面的命令检查配置或直接调用工具。`--doctor` 检查模型接口能否访问、Canvas 令牌和 IMAP 登录，并检查本地浏览器、Python 依赖、已保存的 jAccount 状态和校历。验证当前 jAccount 登录可用 `JIsjtu --sso-login`。

```sh
JIsjtu --doctor
JIsjtu --tool get_time_stamp
JIsjtu --tool find_classroom '{"building":"东上院","date":"tomorrow","start_period":3,"end_period":4}'
```

源码运行时，先从仓库根目录进入 `agent_backend`，再将同样的参数传给程序。

```sh
cd agent_backend
cargo run --locked -- --doctor
cargo run --locked -- --tool get_time_stamp
```

## jAccount 自动登录

打开 TUI 或网页界面时，程序会检查已保存的 jAccount 会话。需要登录时，用 `.env` 中的 `EMAIL_USER_ACCOUNT` 和 `EMAIL_USER_PASSWORD` 在后台浏览器登录，本地用 `ddddocr` 识别图片验证码。交大邮箱地址会自动取 `@` 前的账号部分。要跳过启动检查，可以设置 `AGENT_SSO_AUTO_LOGIN=0`，之后按需运行 `JIsjtu --sso-login`。

使用教务、水源等浏览器工具时，需要本机安装 Edge、Chrome 或 Chromium，以及 Python 3.10+。程序优先使用 Edge。Windows 安装版可从开始菜单运行 **JIsjtu 校园依赖** 配置 Python 环境。macOS 安装版的环境设置见[随包说明](agent_backend/packaging/README.md)。

源码用户可在仓库根目录运行下面的命令，创建独立的 Python 环境。

```sh
python3 -m venv ~/.venvs/jisjtu
~/.venvs/jisjtu/bin/python -m pip install -r agent_backend/requirements-sso.txt
```

在 `agent_backend/.env` 中将 `AGENT_SSO_PYTHON` 指向装好依赖的解释器。上述示例对应 `AGENT_SSO_PYTHON=${HOME}/.venvs/jisjtu/bin/python`。`AGENT_SSO_BROWSER` 可指定浏览器可执行文件，优先于自动查找。

登录状态按账号保存在当前工作目录的 `.sso/`，可用 `AGENT_SSO_DIR` 指定其他私有目录。教务、水源等校园网站复用已保存的登录状态，校外网址使用独立的匿名浏览器配置。Canvas 使用 `CANVAS_API_TOKEN`，选课社区使用自己的一组账号和密码。

想亲自浏览时可以说“打开水源”，想让助手后台查询可以说“查一下水源最近的讨论”。`watch_shuiyuan` 默认展示并保留页面（`mode=show`），`watch_eduinfo` 和 `open_usual_website` 默认返回正文和链接（`mode=read`）。后台查询结束后会清理它创建的临时页面和浏览器。

验证码最多尝试三次，账号密码错误时会停止登录并提示原因，助手仍可启动。需要手动验证时，在 `.env` 设置 `AGENT_SSO_HEADLESS=0`，运行 `JIsjtu --sso-login`。在保留的登录页完成验证后，再运行一次该命令保存会话，然后改回 `AGENT_SSO_HEADLESS=1` 并重启。源码用户在 `agent_backend` 目录运行以下命令。

```sh
cargo run --locked -- --sso-login
```

开发时可在仓库根目录运行 SSO 测试。它使用本地模拟认证服务、临时浏览器配置和模拟账号。

```sh
python3 -m unittest discover -s agent_backend/scripts -p test_sso.py -v
```

## 会话切换

在 TUI 输入 `/new` 新建对话，输入 `/resume`（或 `/sessions`）打开历史会话。用 ↑ / ↓ 选中一条，Enter 打开，`r` 刷新，Esc 返回聊天。按 `d` 或 Delete 会显示待删除会话的标题，按 `y` 永久删除，按 `n` 或 Esc 取消。正在生成回复的会话需等回复结束后再删。删除当前会话会进入新的空白对话。

列表按最近更新时间排序，并标出当前会话和正在处理的会话。模型回复中也可以切换，切回后同步最新内容。每个会话的草稿和滚动位置在本次 TUI 运行期间独立保留。

网页左侧的「我的对话」显示已保存的会话，点击即可切换。「开启new session」会建立独立上下文。切走后任务继续执行，切回时同步完整记录。草稿和滚动位置在当前页面内保留，聊天记录与模型上下文保存在磁盘，刷新或服务重启后仍可继续。

会话由服务端管理，不依赖模型调用文件工具保存。`sessions/<稳定ID>.json` 保存完整模型消息（含工具调用及结果）、页面事件、创建时间和标题；标题取首次提问的前 28 个字符。`sessions/config.json` 是旧的格式示例，不作为聊天记录读取。可通过 `AGENT_SESSIONS_DIR` 指定存储位置，默认使用当前目录或上一级已有的 `sessions/`。

HTTP 接口支持列出会话（`GET /api/sessions`）、新建会话（`POST /api/sessions`）、读取或删除指定会话（`GET` 或 `DELETE /api/sessions/{id}`）。WebSocket `/ws?session_id=<id>` 恢复会话，发送 `{"type":"switch_session","session_id":"<id>"}` 可以切换。服务端通过 `session_snapshot` 和 `session_event` 同步会话内容，在当前打开的会话被删除时发送 `session_deleted`。

开发时可在仓库根目录运行 `node agent_frontend/tests/sessions.cjs` 验证网页会话。脚本使用本地模拟模型，需要 Playwright、浏览器和已构建的后端。默认使用 Edge 和 `agent_backend/target/aarch64-apple-darwin/release/agent`。可用 `BROWSER_CHANNEL`、`PLAYWRIGHT_MODULE` 和 `AGENT_TEST_BINARY` 指定其他浏览器通道、模块和程序路径。

### 让新聊天记住答复偏好

如果这份会话目录只供你个人使用，可以在 `.env` 中设置 `AGENT_DREAMING=1` 并重启。之后，小集会在网页和 TUI 的成功对话结束时，在本机整理你提出的语言、详略和排版偏好。例如，“以后请用中文回答”一次就能生效。在两个不同的聊天分别说“请分点回答”，也会形成默认偏好。当次另有要求时，以当次要求为准。

在聊天里输入 `/memory` 可查看当前生效的偏好，输入 `/forget` 会清空已生效的偏好和用于归纳的记录。删除来源会话也会撤回该会话的依据。明确表达的长期偏好有效 365 天，普通请求的依据有效 90 天。设为 `AGENT_DREAMING=0` 并重启可暂停记录和使用。

跨会话记忆默认关闭，从启用后的新消息开始积累。会话目录下的 `.dreaming/` 仅记录预设的风格选项、来源和时间。整理不调用模型，后续模型请求只附上已形成的风格选项。不同使用者应使用各自的 `AGENT_SESSIONS_DIR`。

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

新增技能或修改名称、描述后请重启。启动日志会列出已加载的技能，恢复历史会话后也能使用更新的清单。

常用校园技能如下。

| 技能 | 用途 |
| --- | --- |
| `sjtu-campus-guide` | 交大常用系统入口、该用哪个工具、校历与节次作息、GPA 4.3 制与目标绩点反推 |
| `course-selection-research` | 按学生关注点比较课程和教师，引用选课社区的评分与分学期点评，按需补充水源讨论或核对课表冲突 |
| `exam-review-plan` | 按真实考试时间与剩余作业倒排复习计划，整理薄弱科目课件重点 |
| `weekly-study-plan` | 结合课表空档、DDL 与考试排一周学习计划，并推荐自习地点 |
| `email-to-teacher` | 起草请假、延期、成绩疑问等邮件，在用户确认后代发 |
| `lecture-notes` | 下载课件、分页阅读，整理概念、公式、易错点与自测题 |

可参考仓库里的[选课调研技能](skills/course-selection-research/SKILL.md)编写自己的技能。

## 从源码运行

项目已在 Rust 1.97.1 验证。macOS 从源码编译还需要 Xcode 命令行工具和 macOS SDK，用于构建 PDF 与文字识别模块。克隆仓库后，从仓库根目录运行以下命令。

```sh
cd agent_backend
test -e .env || cp .env.example .env  # 首次创建配置，已有文件会保留
# 编辑 .env，填写模型服务和要使用的校园功能
cargo run --locked           # 默认 TUI
```

要使用网页界面，在 `agent_backend` 目录运行 `cargo run --locked -- --web`。保留同级的 `agent_frontend` 目录，修改网页文件后刷新即可查看效果。

## 构建安装包

### Windows x64

在仓库根目录运行下列命令。在 macOS 或 Linux 构建机上准备 Rust 的 `x86_64-pc-windows-gnu` 目标、MinGW-w64 编译器及 binutils、CMake、Make、curl、tar 和 Python 3。构建脚本会获取并校验 PDFium、Tesseract 和中英文识别模型，最终生成根目录的 `JIsjtu-Windows-x64.zip`。Windows 用户只需解压安装。用于开发时，可将 `AGENT_PDF_TOOLS_DIR` 指向解压目录中的 `pdf-tools`。

```sh
rustup target add x86_64-pc-windows-gnu
python3 agent_backend/scripts/package-windows.py
```

可在 Windows PowerShell 中运行 `powershell -NoProfile -ExecutionPolicy Bypass -File .\agent_backend\scripts\test-windows-package.ps1`，在临时目录验证安装包。要验证随包中英文 OCR 和模型收图，可运行 `py -3 agent_backend/scripts/test-windows-pdf.py "$env:LOCALAPPDATA/JIsjtu/agent.exe"`。测试使用本机模拟模型。

### macOS 和 Linux

在 `agent_backend` 目录运行：

```sh
sh scripts/package.sh
```

输出 `agent_backend/dist/JIsjtu-<平台>.zip`，同时更新仓库根目录的 `JIsjtu.zip` 和 `install.sh`。默认包包含编译好的程序、网页界面、上表中的六项校园技能、新手教程、`.env.example`、SSO 依赖清单和许可证。macOS 包另外提供可双击的 `Install.command`。压缩包内的 `install.sh` 和仓库根目录的安装脚本由同一模板生成，根目录脚本也可安装本机已构建的 release。

如需分发自己的技能集，将 `AGENT_PACKAGE_SKILLS_DIR` 设为该目录的路径，打包时会一并复制其中的技能和参考文件。

已有经过验证的当前平台 release 时，可直接打包该文件：

```sh
AGENT_PACKAGE_BINARY="$PWD/target/release/agent" sh scripts/package.sh
```

默认按当前系统及架构构建；Intel Mac、Apple Silicon Mac、Linux 需要分别构建。Python 和浏览器需在目标机器安装，SSO 依赖按包内说明安装。Linux 包仍依赖相应系统动态库。

构建本机的 release 和安装包后，在 `agent_backend` 目录运行下面的命令。脚本会校验 ZIP 内容，并在临时目录验证安装、升级和配置保留。

```sh
python3 scripts/test-package.py --archive ../JIsjtu.zip
```

安装脚本支持 `--prefix /绝对路径`、`--no-path`、`--profile /配置文件路径`。

## 使用边界

这是个人本地助手，HTTP 与 TCP 默认只监听 `127.0.0.1`。设置 `AGENT_BIND_ADDR=0.0.0.0` 会同时向所有 IPv4 网卡开放两个端口。接口没有用户认证，能连入的人可以调用本机工具并使用已登录的校园服务，请仅在受控网络中按需启用。macOS 分发程序未经 Developer ID 签名或公证。

## 许可证

[MIT](LICENSE)，Copyright (c) 2026 XIAOMIQ772。
