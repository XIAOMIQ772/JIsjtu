# JIsjtu · 交我集

上海交通大学校园信息查询助手兼学习工作小助手。支持通过自然语言查询 Canvas 课程、作业、DDL、公告与成绩，读取教务系统课表、考试安排和 GPA，找空教室、看图书馆人数，搜索和阅读水源帖子，读取与搜索校园邮件、阅读 PDF、帮写并提交作业，也可运行命令、处理文件和完成编码任务，支持url/名称唤起任意网站

现已支持多sessions热切换，skills读取，以及交大sso自动化登陆and持久化使用

建议使用deepseek相关模型以提升运行速度

前端使用 HTML / CSS / JavaScript，后端使用 Rust + Axum，通过 WebSocket 传递回答与工具进度。`JIsjtu` 和源码运行的 `cargo run` 都默认使用终端界面；显式指定 `--web` 或 `web` 才打开网页，无需单独启动前端。

更新内容见 [CHANGELOG.md](CHANGELOG.md)。

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

`JIsjtu` 默认启动 TUI，可输入 `/sessions` 切换会话，`/today` 看今日简报、`/ddl` 汇总作业截止、`/week` 看本周课表（输入 `/` 可打开命令面板）。运行 `JIsjtu --web` 或 `JIsjtu web` 才会打开网页，默认地址为 `http://127.0.0.1:13376`。终端需保持运行，按 `Ctrl+C` 停止。`JIsjtu --config` 可查看配置路径，`JIsjtu --tui` 显式使用终端界面，`JIsjtu --sso-login` 单独检查登录。配置修改或升级后需重启。

## 配置

配置示例见 [agent_backend/.env.example](agent_backend/.env.example)。个人 `.env` 不提交到 Git，也不打进分发包。

| 变量 | 用途 |
| --- | --- |
| `OPENAI_BASE_URL` | OpenAI 兼容服务的接口地址； |
| `OPENAI_API_KEY` | 模型服务密钥 |
| `MODEL` | 模型名称 |
| `CANVAS_API_TOKEN` | 在 Canvas 的「账户 → 设置」创建访问令牌，用于课程、作业、课件和课堂签到记录查询 |
| `EMAIL_USER_ACCOUNT` / `EMAIL_USER_PASSWORD` | jAccount 账号或交大邮箱地址，以及 jAccount 登录密码 |
| `IMAP_PASSWORD` | 可选，独立的邮箱密码或客户端授权码；留空时沿用 `EMAIL_USER_PASSWORD` |
| `IMAP_HOST` / `IMAP_PORT` | 邮箱服务器；学校邮箱可设置 `imap.sjtu.edu.cn`、`993` |
| `SMTP_HOST` / `SMTP_PORT` / `SMTP_PASSWORD` | 发信默认使用 `mail.sjtu.edu.cn:465`（TLS）。未单设密码时依次使用 `IMAP_PASSWORD`、`EMAIL_USER_PASSWORD` |
| `AGENT_BIND_ADDR` | HTTP 和 TCP 共用的监听 IP，默认 `127.0.0.1`。需要局域网访问时可指定本机的局域网 IP |
| `AGENT_HTTP_PORT` / `AGENT_TCP_PORT` | HTTP 与 TCP 端口，默认 `13376` / `8081` |
| `AGENT_REMINDERS` | macOS 运行期间提醒上课与作业截止，设为 `0` 可关闭 |
| `AGENT_NO_BROWSER` | 设置此变量可跳过自动打开浏览器 |
| `AGENT_FRONTEND_DIR` | 可选，指定前端静态文件目录 |
| `AGENT_SKILLS_DIR` | 可选，指定技能目录；默认找当前目录或上一级的 `skills/` |
| `SJTU_TERM_START` / `SJTU_TERM_WEEKS` / `SJTU_TERM_NAME` | 可按教务处校历填写第 1 周周一（YYYY-MM-DD）、总周数（默认 18）和学期名称 |

模型配置用于对话；Canvas 和邮箱配置按需填写。

## 校园功能

除了基础的文件、命令与网页工具，小集内置了以下校园查询，均可直接用自然语言触发：

| 想做的事 | 工具 | 数据来源 |
| --- | --- | --- |
| 今天有什么安排 | `daily_brief`：今天（晚上附明天）的课、14 天内考试、48 小时内截止和逾期作业、24 小时内公告、未读邮件，并发获取 | 教务系统、Canvas、邮箱 |
| 课表、下节课在哪 | `class_schedule`：今天/明天/本周/下周/整学期，按教学周与单双周过滤 | 教学信息服务网 |
| 考试时间与考场 | `exam_schedule`：倒计时、考场、未公布标注 | 教学信息服务网 |
| 成绩、GPA 和排名 | `score_report` 根据本人成绩估算总 GPA、学期 GPA 和加权均分。`view=official` 提供官方本人查询入口及学院排名证明办理方式 | 教学信息服务网、学院办事指南 |
| 离毕业还差什么 | `degree_progress`：培养方案各类要求的要求/已获得学分与缺口 | 教学信息服务网 |
| 作业截止 | `get_deadlines`（跨课程）、`get_exam`（单门课，含提交状态） | Canvas |
| 课程公告 | `get_announcements` | Canvas |
| 平时成绩与评语 | `get_grades`：逐项得分、班级均分/最高/最低、权重、老师评语 | Canvas |
| 找空教室 | `find_classroom`：按教学周算空闲节次，附教室实时人数；也能查某间教室当天课表 | 教室信息服务（公开） |
| 图书馆人多吗 | `library_status`：各馆在馆人数与占比 | 图书馆人数统计（公开） |
| 水源 | `get_hotnews` 热帖简报、`shuiyuan_search` 搜索、`shuiyuan_topic` 读帖子、`shuiyuan_post` 查看版块、发表主题或回帖 | 水源社区 |
| 邮件 | `mail_fetch` 列出、阅读、搜索、下载附件，`send_email` 发送纯文本邮件 | 学校邮箱 |
| 找课件、下载整门课 | `search_files` 跨课程搜索，`course_files` 的 `download_all` 下载指定课程中可访问的全部文件，默认目录为 `~/Downloads/Canvas-<课程id>`。文件名带 Canvas ID，重跑会跳过已有且大小匹配的文件 | Canvas |
| 读课件、文档 | `pdf_analysis` 读取 PDF，在 macOS 上自动用 Vision 离线识别扫描页。`read_office` 读取 pptx（含讲者备注）、docx（含表格）和 xlsx | 本地文件 |
| 上课、作业主动提醒 | 程序运行期间，macOS 在上课前 10 分钟、未完成作业截止前 1 小时发送通知。课表需要已登录的教务会话，作业需要 Canvas 令牌 | 教务系统、Canvas |
| 盯一节课的签到 | 对小集说“帮我盯明早 8 点的大学物理有没有发起签到”。`watch_attendance` 为这一节课安排监看，在 Canvas「点名签到」里发现时段内的新记录就发 macOS 通知 | Canvas 课堂签到 |

例如 08:00 开始的课，签到监看在北京时间 07:50 至 08:05 每 30 秒刷新一次。中途提出请求会从当前时刻开始。监看期间请保持 Mac 唤醒并运行 JIsjtu。每条新记录提醒一次，有本人签到状态时一并显示。监看期间如始终无法读取记录，结束时会收到失败提醒。程序重启后会恢复尚未过期的任务。
| 导入手机日历 | `export_calendar`：课表按周展开、考试、未交作业截止导出为 .ics，带提醒 | 教务系统、Canvas |

所有时间按北京时间输出。内置 2026–2027 秋季学期从 9 月 14 日开始，共 18 周，第 17–18 周为考试周。第 1 节课 08:00 开始，每节 45 分钟。截至 2026 年 11 月 26 日，教务处校历目录仅列出该学年的秋季学期。春季校历公布后，可按官方日期在 `.env` 填写 `SJTU_TERM_START` 和 `SJTU_TERM_WEEKS`。[教务处历年校历](https://jwc.sjtu.edu.cn/jxxl/lnxl.htm)

教务系统和水源需要 jAccount 登录态：浏览器助手先在后台打开站点入口页走完单点登录，再在同一页面里请求查询接口，多项查询共用一次浏览器会话；jAccount 会话过期时会自动重新登录一次。

同一轮里模型发出的多个只读查询会并发执行，结果仍按调用顺序返回；命令、写文件、提交作业等有副作用的调用保持顺序执行。

代发邮件或水源帖子时，先核对会话中显示的收件人、版块或主题和完整正文，然后在同一会话回复预览给出的 `确认发送 <口令>`。口令 15 分钟内有效，每个会话只保留最新一份待发草稿。

配置好后可以先自检，逐项确认模型、Canvas、邮箱、Python 依赖、浏览器、jAccount 与校历是否可用：

```sh
JIsjtu --doctor                 # 安装包
cargo run --locked -- --doctor  # 源码
```

调试时可以不经过模型直接调用某个工具：

```sh
cargo run --locked -- --tool get_time_stamp
cargo run --locked -- --tool find_classroom '{"building":"东上院","date":"tomorrow","start_period":3,"end_period":4}'
```

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

在 TUI 输入 `/resume`（或 `/sessions`）打开历史会话。用 ↑ / ↓ 选中一条，Enter 打开，`r` 刷新，Esc 返回聊天。按 `d` 或 Delete 会显示待删除会话的标题，按 `y` 永久删除，按 `n` 或 Esc 取消。正在生成回复的会话需等回复结束后再删。删除当前会话会进入新的空白对话。

列表按最近更新时间排序，并标出当前会话和正在处理的会话。模型回复中也可以切换，切回后同步最新内容。每个会话的草稿和滚动位置在本次 TUI 运行期间独立保留。

会话由服务端管理，不依赖模型调用文件工具保存。`sessions/<稳定ID>.json` 保存完整模型消息（含工具调用及结果）、页面事件、创建时间和标题；标题取首次提问的前 28 个字符。`sessions/config.json` 是旧的格式示例，不作为聊天记录读取。可通过 `AGENT_SESSIONS_DIR` 指定存储位置，默认使用当前目录或上一级已有的 `sessions/`。

HTTP 接口支持列出会话（`GET /api/sessions`）、新建会话（`POST /api/sessions`）、读取或删除指定会话（`GET` 或 `DELETE /api/sessions/{id}`）。WebSocket `/ws?session_id=<id>` 恢复会话，发送 `{"type":"switch_session","session_id":"<id>"}` 可以切换。服务端通过 `session_snapshot` 和 `session_event` 同步会话内容，在当前打开的会话被删除时发送 `session_deleted`。

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

技能只在启动时扫描，改完要重启服务。

仓库自带的技能：

| 技能 | 用途 |
| --- | --- |
| `sjtu-campus-guide` | 交大常用系统入口、该用哪个工具、校历与节次作息、GPA 4.3 制与目标绩点反推 |
| `course-selection-research` | 选课调研：水源多关键词检索、精读帖子、核对课表冲突，给出有出处的对比 |
| `exam-review-plan` | 按真实考试时间与剩余作业倒排复习计划，整理薄弱科目课件重点 |
| `weekly-study-plan` | 结合课表空档、DDL 与考试排一周学习计划，并推荐自习地点 |
| `email-to-teacher` | 起草请假、延期、成绩疑问等邮件，在用户确认后代发 |
| `lecture-notes` | 下载课件、分页阅读，整理概念、公式、易错点与自测题 |格式细节和目录查找顺序见 [skills/README.md](skills/README.md)。

## 从源码运行

需要支持项目所用 API 的近期稳定版 Rust（已在 Rust 1.97.1 验证），无需 Node。克隆项目后：

```sh
cd agent_backend
cp .env.example .env
# 编辑 .env，填写个人配置
cargo run --locked           # 默认 TUI
cargo run --locked -- --web  # 显式启动网页界面
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

`--smoke` 在临时目录安装，检查真实程序和安装命令默认进入 TUI，并验证显式 Web 模式的 HTTP、静态文件与 TCP 服务，不修改真实 shell 配置。安装脚本支持 `--prefix /绝对路径`、`--no-path`、`--profile /配置文件路径`。

## 使用边界

这是个人本地助手，HTTP 与 TCP 默认只监听 `127.0.0.1`。设置 `AGENT_BIND_ADDR=0.0.0.0` 会同时向所有 IPv4 网卡开放两个端口。接口没有用户认证，能连入的人可以调用本机工具并使用已登录的校园服务，请仅在受控网络中按需启用。macOS 分发程序未经 Developer ID 签名或公证。

## 许可证

[MIT](LICENSE)，Copyright (c) 2026 XIAOMIQ772。
