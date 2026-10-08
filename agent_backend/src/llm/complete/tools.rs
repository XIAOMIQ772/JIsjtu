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
                "当用户给出课程名字时,先去查课程id,然后查询课程对应的作业描述,展示课程的名称,截止时间,分值,内容".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "course_id":{
                        "type":"string",
                        "description":"课程id,每门课程id独一无二"
                    }
                },
                "required":["course_id"],
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
            name: "get_time_stamp".to_string(),
            description: Some(
                "获取当前系统时间".to_string(),
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
                "通过 IMAP 读取学校邮箱。可以列出某个文件夹里的邮件、读取某一封的正文、\
                 或列出所有文件夹。只读，不会改动已读状态。".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "action":{
                        "type":"string",
                        "enum":["list","read","folders"],
                        "description":"list=列出邮件;read=读某一封的正文;folders=列出所有文件夹"
                    },
                    "folder":{
                        "type":"string",
                        "description":"文件夹名，默认 INBOX。用 action=folders 查看有哪些"
                    },
                    "limit":{
                        "type":"integer",
                        "description":"list 时最多返回几封，默认 10，上限 50"
                    },
                    "uid":{
                        "type":"integer",
                        "description":"action=read 时必填，来自 list 结果里的 UID"
                    },
                    "unread_only":{
                        "type":"boolean",
                        "description":"list 时只列未读邮件"
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
