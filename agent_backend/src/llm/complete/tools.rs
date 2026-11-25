use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use serde_json::json;
pub fn get_tools() -> Vec<ChatCompletionTools> {
    vec![
    ChatCompletionTools::Function(ChatCompletionTool {
        function: FunctionObject {
            name: "sso_login".to_string(),
            description: Some(
                "检查并登录交大 jAccount 单点登录。程序从 .env 的 EMAIL_USER_ACCOUNT、EMAIL_USER_PASSWORD 读取凭据，\
                 默认在后台无窗口运行，本地 OCR 识别图片验证码，成功后保存浏览器会话并关闭登录页。启动时自动运行；登录过期或失败后可重试。\
                 账号密码错误时先让用户检查配置，不要重复尝试。不要在工具参数或对话里传入账号、密码、Cookie；\
                 后续校园网页工具默认在后台复用该会话。".to_string(),
            ),
            parameters: Some(json!({"type":"object", "properties":{}, "required":[], "additionalProperties":false})),
            strict: Some(true),
        },
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "bash".to_string(),
            description: Some(
                "在 bash 中执行命令，返回合并的 stdout/stderr。长时间任务放后台运行。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "command":{
                        "type":"string",
                        "description":"需要执行的命令"
                    }
                },
                "required":["command"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "read".to_string(),
            description: Some(
                "读文件，带行号返回。路径是目录则列出内容。大文件用 offset/limit 分段读。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "path":{
                        "type":"string",
                        "description":"绝对路径"
                    },
                    "offset":{
                        "type":"integer",
                        "description":"起始行数,从1开始"
                    },
                    "limit":{
                        "type":"integer",
                        "description":"读取行数"
                    }
                },
                "required":["path"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "write".to_string(),
            description: Some(
                "写入文件，已存在则覆盖，父目录自动创建。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "path":{
                        "type":"string",
                        "description":"绝对路径,父目录自动创建"
                    },
                    "content":{
                        "type":"string",
                        "description":"要写入的完整内容"
                    }
                },
                "required":["path","content"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "edit".to_string(),
            description: Some(
                "替换文本中已存在的一段内容".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "path":{
                        "type":"string",
                        "description":"绝对路径"
                    },
                    "old_context":{
                        "type":"string",
                        "description":"要替换的原文,需唯一"
                    },
                    "new_context":{
                        "type":"string",
                        "description":"要替换的新文本"
                    }
                },
                "required":["path","old_context","new_context"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_courses".to_string(),
            description: Some(
                "查询当前账号在 Canvas@SJTU 上进行中的课程，返回课程 ID、名称与学期".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{},
                "required":[],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_exam".to_string(),
            description: Some(
                "查询某门课程在 Canvas 上的全部作业：截止时间（北京时间，附剩余时间）、自己的提交/批改状态、分值、提交方式和说明摘要，\
                 按未截止、无截止时间、已截止分组。用户只给课程名时先用 get_courses 查课程 id。只关心还没交的作业时传 pending_only=true。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "course_id":{
                        "type":"string",
                        "description":"课程 id（数字），可由 get_courses 获取"
                    },
                    "pending_only":{
                        "type":["boolean","null"],
                        "description":"true 只列出还需要提交的作业；null 或 false 列出全部"
                    }
                },
                "required":["course_id","pending_only"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_deadlines".to_string(),
            description: Some(
                "一次汇总所有课程近期的截止事项（Canvas 作业、测验、讨论、日程和个人待办），按截止时间排序，\
                 标出今天/明天、剩余时间、是否已提交，并列出最近两周已截止但还没交的作业。\
                 用户问“最近有什么 DDL”“这周要交什么”“还有哪些作业没写”时首选它，不必逐门课调用 get_exam。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "days":{"type":["integer","null"],"description":"向后看多少天，1 到 60，null 表示默认 14 天；问“这周”可传 7"},
                    "include_completed":{"type":["boolean","null"],"description":"true 时也列出窗口内已完成的事项；默认只给数量"}
                },
                "required":["days","include_completed"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_announcements".to_string(),
            description: Some(
                "读取 Canvas 课程公告（调课、考试安排、成绩公布、作业说明等），按发布时间从新到旧，带课程名、发布人、是否未读和附件。\
                 不传 course_id 时汇总所有进行中的课程。用户问“老师最近发了什么通知”“有没有调课/考试通知”时使用。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "course_id":{"type":["string","null"],"description":"只看某门课时传课程 id，null 表示所有课程"},
                    "days":{"type":["integer","null"],"description":"看最近多少天的公告，1 到 180，null 表示默认 14 天"}
                },
                "required":["course_id","days"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_grades".to_string(),
            description: Some(
                "查询 Canvas 上的作业成绩与老师反馈。不传 course_id：各课程当前总评（老师公开时）和最近 30 天新批改的作业；\
                 传 course_id：该课逐项得分、班级均分/最高/最低、分组权重与得分率、老师和助教的评语。\
                 这是 Canvas 平时成绩；期末总评和绩点以教务系统为准。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "course_id":{"type":["string","null"],"description":"课程 id；null 表示看所有课程的概览"}
                },
                "required":["course_id"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "library_status".to_string(),
            description: Some(
                "查询交大各图书馆（主馆、李政道图书馆、包玉刚图书馆、徐汇社科馆）的实时在馆人数和容量占比，\
                 用户问“图书馆人多吗”“去哪个馆自习”时使用。无需登录。".to_string(),
            ),
            parameters: Some(json!({"type":"object","properties":{},"required":[],"additionalProperties":false})),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "find_classroom".to_string(),
            description: Some(
                "找空教室/自习室：根据学校教室系统的排课，按教学周过滤后算出各教室的空闲节次，今天还附带教室传感器的实时人数。\
                 building 传教学楼名（上院、中院、下院、东上院、东中院、东下院、陈瑞球楼、新上院、教一楼、工程馆，可简写如“东上”），\
                 传具体教室（如“东上院102”）时返回该教室当天的完整排课；传校区名或 null 时汇总各楼空教室数量。\
                 不传节次时从当前节次开始，按连续空闲时长排序；同时传 start_period 和 end_period 时只返回整段都空的教室。\
                 节次对应时间：1-2 节 08:00–09:40，3-4 节 10:00–11:40，5-6 节 12:00–13:40，7-8 节 14:00–15:40，9-10 节 16:00–17:40，11-12 节 18:00–19:40，13-14 节 20:00–21:40。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "building":{"type":["string","null"],"description":"教学楼、具体教室或校区（闵行/徐汇）；null 汇总所有楼"},
                    "date":{"type":["string","null"],"description":"today、tomorrow 或 YYYY-MM-DD，null 表示今天"},
                    "start_period":{"type":["integer","null"],"description":"起始节次 1–14；null 表示今天从当前节次开始、其他日期从第 1 节开始"},
                    "end_period":{"type":["integer","null"],"description":"结束节次 1–14；给出时只返回整段空闲的教室"}
                },
                "required":["building","date","start_period","end_period"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "watch_shuiyuan".to_string(),
            description: Some(
                "打开水源社区，复用 jAccount 登录状态。默认 show：打开可见网页并保留给用户。只有用户让助手查询、阅读、提取或总结网页内容时，才明确传 read，在后台读取后自动关闭临时页面。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "mode":{"type":["string","null"],"enum":["read","show",null],"description":"show 或 null（默认）：打开可见网页并保留给用户；read：仅在用户让助手查询或阅读内容时使用，后台读取后关闭临时页面"}
                },
                "required":["mode"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_hotnews".to_string(),
            description: Some(
                "抓取水源社区最新与本周热门帖子，按点赞、回复、浏览量和时效计算热度，返回热点帖子简报（含链接、数据及前几名的楼主内容与热评）。用户想看看水源热点、热帖、最近水源在讨论什么时调用；只想打开水源网页时用 watch_shuiyuan。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "limit":{"type":["integer","null"],"description":"返回的热点帖子数量，1 到 20，null 表示默认 8"}
                },
                "required":["limit"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "shuiyuan_search".to_string(),
            description: Some(
                "在水源社区搜索帖子，返回相关帖子（标题、链接、楼数、点赞、时间）及命中楼层摘要。\
                 适合查选课评价、老师口碑、给分情况、考试经验、校园生活问题（如“高数 选课”“某老师 给分”“保研 经验”）。\
                 支持 Discourse 搜索语法：order:latest 按时间排序、in:title 只搜标题、after:2025-01-01 限定时间。\
                 要读完整讨论用 shuiyuan_topic。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string","description":"搜索词，至少两个字；用课程简称、老师姓名等关键词效果更好"},
                    "limit":{"type":["integer","null"],"description":"最多返回几个帖子，1 到 20，null 表示 10"}
                },
                "required":["query","limit"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "shuiyuan_topic".to_string(),
            description: Some(
                "阅读水源上的某个帖子：标题、楼主正文和各楼回复（作者、时间、点赞、回复关系），每次约 20 楼，\
                 可用 start 往后翻页。帖子 id 来自 shuiyuan_search 或 get_hotnews，也可以直接传帖子链接。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "topic_id":{"type":"string","description":"帖子 id 或帖子链接"},
                    "start":{"type":["integer","null"],"description":"从第几楼开始读，null 表示从 1 楼开始"}
                },
                "required":["topic_id","start"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "daily_brief".to_string(),
            description: Some(
                "今日简报：一次并发汇总今天的课（晚上附明天的课）、14 天内的考试、48 小时内截止和逾期未交的作业、\
                 最近 24 小时的课程公告、未读邮件。用户说“早上好”“今天有什么安排”“帮我看看今天”“最近要忙什么”时首选它，\
                 再按需用其他工具查细节。".to_string(),
            ),
            parameters: Some(json!({"type":"object","properties":{},"required":[],"additionalProperties":false})),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_time_stamp".to_string(),
            description: Some(
                "获取当前北京时间、星期、交大校历的教学周（是否考试周、节假日）以及当前处于第几节课。\
                 涉及“今天/明天/本周/第几周/还有几天”的问题先调用它。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                },
                "required":[],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "pdf_analysis".to_string(),
            description: Some(
                "读取 PDF 文件中的文字。页数少的会一次返回全文；页数多时先返回各页开头作为目录，\
                 再指定 start_page / end_page 读取需要的页面。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "path":{
                        "type":"string",
                        "description":"pdf文件的绝对路径"
                    },
                    "start_page":{
                        "type":"integer",
                        "description":"起始页码,从1开始。不传表示从头开始"
                    },
                    "end_page":{
                        "type":"integer",
                        "description":"结束页码,含该页。不传表示读到最后一页"
                    }
                },
                "required":["path"],
                "additionalProperties":false
            })),
            strict: Some(false),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "course_files".to_string(),
            description: Some(
                "查看某门课程在 Canvas 上的文件（讲义、课件、作业附件等）。\
                 可以列出文件、把文件下载到本地、或下载后用系统默认程序打开。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "course_id":{
                        "type":"string",
                        "description":"课程 id，可由 get_courses 获取"
                    },
                    "action":{
                        "type":"string",
                        "enum":["list","download","open"],
                        "description":"list=列出文件；download=下载供后台分析；open=下载并展示给用户，仅在用户明确要查看文件时使用。为查询或分析资料应下载后调用读取工具。"
                    },
                    "file":{
                        "type":"string",
                        "description":"文件名或其一部分，也可以直接给文件 id。action 为 download/open 时必需"
                    },
                    "save_path":{
                        "type":"string",
                        "description":"保存的完整路径，不传则存到 ~/Downloads/<原文件名>"
                    }
                },
                "required":["course_id","action"],
                "additionalProperties":false
            })),
            strict: Some(false),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "homework".to_string(),
            description: Some(
                "帮用户写作业并提交到 Canvas。action=questions：从作业来源中准确切分题目，返回每道题的编号、页码、分值、小问和完整题干；\
                 来源三选一：path=本地 PDF 或文本文件，text=作业只是一段文字时直接传入，course_id+assignment_id=Canvas 作业（自动读取说明并下载其中的 PDF 附件）。\
                 题目较多时先返回目录，再用 question 传序号读取单题。写作业时先用它拿到题目再逐题作答。\
                 action=submit：text 为在线文本提交，files 为上传本地文件提交，二者只能选一。\
                 用户已经明确说可以提交（如“写完直接交”“提交吧”）时，传 confirm=true 直接提交；\
                 用户没有明确说要提交时，传 confirm=false 生成预览，系统会把预览直接展示给用户，\
                 等用户回复同意后再用 confirm=true 提交预览过的那份内容。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "action":{
                        "type":"string",
                        "enum":["questions","submit"],
                        "description":"questions=识别题目；submit=提交作业"
                    },
                    "path":{
                        "type":"string",
                        "description":"questions：作业 PDF 或文本文件的绝对路径"
                    },
                    "text":{
                        "type":"string",
                        "description":"questions：没有 PDF 时的作业题目文字；submit：在线文本提交的答案，纯文本会自动转为 HTML"
                    },
                    "course_id":{
                        "type":"string",
                        "description":"课程 id，可由 get_courses 获取。submit 时必填"
                    },
                    "assignment_id":{
                        "type":"string",
                        "description":"作业 id，可由 get_exam 获取。submit 时必填"
                    },
                    "question":{
                        "type":"string",
                        "description":"questions：只读取某一题，传序号（从 1 开始）或题号如 Problem 3"
                    },
                    "files":{
                        "type":"array",
                        "items":{"type":"string"},
                        "description":"submit：要上传的本地文件绝对路径"
                    },
                    "comment":{
                        "type":"string",
                        "description":"submit：附带给老师的留言，可选"
                    },
                    "confirm":{
                        "type":"boolean",
                        "description":"submit：用户已明确同意提交时传 true，直接提交；否则传 false 只生成预览。预览后用户同意再传 true 时只需 course_id 与 assignment_id"
                    }
                },
                "required":["action"],
                "additionalProperties":false
            })),
            strict: Some(false),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "watch_eduinfo".to_string(),
            description: Some(
                "读取教学信息服务网的页面内容和链接，供后续课程查询使用。查课程时用 read，默认后台读取后自动关闭临时页面；仅用户明确要看网页时使用 show。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "mode":{"type":["string","null"],"enum":["read","show",null],"description":"read 或 null：后台读取并关闭临时页面；show：展示并保留给用户"}
                },
                "required":["mode"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "class_schedule".to_string(),
            description: Some(
                "从教学信息服务网（教务系统）读取本人课表，按交大校历的教学周和单双周过滤，给出节次对应时间、教室和老师。\
                 day：today（默认）、tomorrow、this_week、next_week、all（整学期课表）或 YYYY-MM-DD；week 可直接指定第几教学周。\
                 term 查其他学期，格式 2025-2026-2（1 秋季、2 春季、3 夏季），非当前学期时给整学期课表。\
                 用户问“今天/明天有什么课”“下节课在哪”“这周课表”时使用。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "day":{"type":["string","null"],"description":"today、tomorrow、this_week、next_week、all 或 YYYY-MM-DD；null 表示今天"},
                    "week":{"type":["integer","null"],"description":"指定教学周（1–18），给出时返回该周课表"},
                    "term":{"type":["string","null"],"description":"学期，如 2025-2026-2；null 表示当前学期"}
                },
                "required":["day","week","term"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "exam_schedule".to_string(),
            description: Some(
                "从教务系统读取本人考试安排（期中、期末等）：日期、星期、起止时间、倒计时、考场与校区，按时间排序，\
                 未公布的考试会标注。用户问“什么时候考试”“考场在哪”“还有几天考试”时使用。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "term":{"type":["string","null"],"description":"学期，如 2026-2027-1；null 表示当前学期"}
                },
                "required":["term"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "score_report".to_string(),
            description: Some(
                "从教务系统读取本人各学期课程成绩与绩点，按学分加权计算总 GPA、各学期 GPA 和加权均分（P/F 课程不计入绩点），\
                 并列出每门课的学分、成绩、绩点、考试性质。用户问“我的 GPA”“这学期成绩出了吗”“哪门课分低”时使用。\
                 Canvas 上的平时作业分数用 get_grades。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "term":{"type":["string","null"],"description":"只看某学期，如 2025-2026-2；null 表示全部学期"}
                },
                "required":["term"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: FunctionObject {
            name: "open_usual_website".to_string(),
            description: Some(
                "访问指定 HTTP(S) 网页。查询任务用 read，后台返回可见正文和链接，完成后自动关闭临时页面，可继续读取返回的链接；校园网页复用 jAccount 登录。show 仅用于用户明确要求打开网页查看，此时展示并保留页面。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "url":{"type":"string","description":"需要访问的完整 HTTP(S) 网页地址"},
                    "mode":{"type":["string","null"],"enum":["read","show",null],"description":"查询用 read 或 null；仅用户明确要看网页时用 show"}
                },
                "required":["url","mode"],
                "additionalProperties":false
            })),
            strict: Some(true),
        },
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "mail_fetch".to_string(),
            description: Some(
                "通过 IMAP 读取学校邮箱（只读，不改变已读状态）。list 列出某文件夹最新邮件；read 读某一封正文并列出带序号的附件；\
                 search 按关键词匹配最近邮件的发件人和主题（如“奖学金”“Canvas”“讲座”“教务处”）；\
                 attachment 把某封邮件的附件保存到本地（默认 ~/Downloads），之后可用 pdf_analysis 或 read 阅读；folders 列出文件夹。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "action":{
                        "type":"string",
                        "enum":["list","read","search","attachment","folders"],
                        "description":"list=列出邮件；read=读正文；search=按关键词找邮件；attachment=下载附件；folders=列出文件夹"
                    },
                    "folder":{"type":"string","description":"文件夹名，默认 INBOX"},
                    "limit":{"type":"integer","description":"list/search 最多返回几封，默认 10，上限 50"},
                    "uid":{"type":"integer","description":"read/attachment 时必填，来自 list 或 search 结果里的 UID"},
                    "unread_only":{"type":"boolean","description":"list 时只列未读邮件"},
                    "keyword":{"type":"string","description":"search 的关键词，匹配发件人或主题"},
                    "scan":{"type":"integer","description":"search 时检查最近多少封邮件，默认 200，上限 2000"},
                    "attachment":{"type":"string","description":"attachment 时要下载的附件序号（从 1 开始）或文件名片段，默认第 1 个"},
                    "save_path":{"type":"string","description":"attachment 时保存的完整路径，不传则存到 ~/Downloads/<附件名>"}
                },
                "required":["action"],
                "additionalProperties":false
            })),
            strict: Some(false),
        }),
    }),
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "create_subagents".to_string(),
            description: Some(
                "将可独立完成的子任务并发分发给多个子 agent，等待全部完成后按输入顺序返回各自的结果或错误。\
                 子 agent 可以使用其他工具，但不能再次创建子 agent。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "number":{
                        "type":"integer",
                        "minimum":1,
                        "description":"需要创建的子 agent 数量，必须与 roles、tasks、messages 的长度一致"
                    },
                    "roles":{
                        "type":"array",
                        "description":"每个子 agent 的角色，按下标与任务、上下文一一对应",
                        "items":{"type":"string","minLength":1}
                    },
                    "tasks":{
                        "type":"array",
                        "description":"每个子 agent 需要完成的具体任务",
                        "items":{"type":"string","minLength":1}
                    },
                    "messages":{
                        "type":"array",
                        "description":"每个子 agent 应知晓的上下文，不需要上下文时该项填 null",
                        "items":{"type":["string","null"]}
                    }
                },
                "required":["number","roles","tasks","messages"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
    })]
}
