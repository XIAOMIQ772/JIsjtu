use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use serde_json::json;
pub fn get_tools() -> Vec<ChatCompletionTools> {
    vec![
    ChatCompletionTools::Function(ChatCompletionTool {
        function: (FunctionObject {
            name: "get_num".to_string(),
            description: Some(
                "when user ask questions about somethings' number ,use this tool".to_string(),
            ),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "password":{
                        "type":"string",
                        "description":"check id about who use this tool"
                    }
                },
                "required":["password"],
                "additionalProperties":false
            })),
            strict: Some(true),
        }),
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
                "打开名为水源社区的网页".to_string(),
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
                        "description":"list=列出文件;download=下载到本地;open=下载并用系统默认程序打开"
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
                "打开教学信息服务网".to_string(),
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
    })]
}
