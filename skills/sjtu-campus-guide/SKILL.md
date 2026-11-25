---
name: sjtu-campus-guide
description: 交大校园常识速查：常用系统入口与用途、校历与作息节次、GPA 4.3 制换算、各类校园问题该用哪个工具。回答“去哪里办/查”“第几节几点”“绩点怎么算”这类问题前先读它。
---

# 交大校园常识速查

本文件是事实依据。回答时以这里和工具返回的数据为准，不要凭印象编造网址、时间或规则；这里没有的内容，用 open_usual_website（mode=read）去官网核实后再答，并告诉用户信息来源。

## 一、常用系统入口

- Canvas（课程、作业、课件、公告、平时成绩）：https://oc.sjtu.edu.cn
- 教学信息服务网（课表、考试安排、成绩、选课、评教）：https://i.sjtu.edu.cn
- 教务处（教务通知、校历、学籍与选课政策）：https://jwc.sjtu.edu.cn
  - 通知通告：https://jwc.sjtu.edu.cn/xwtg/tztg.htm
  - 本年校历：https://jwc.sjtu.edu.cn/jxxl/bnxlst.htm
- 水源社区（同学交流、选课评价、二手、失物招领）：https://shuiyuan.sjtu.edu.cn
- 交大邮箱：https://mail.sjtu.edu.cn
- 图书馆：https://www.lib.sjtu.edu.cn（注意要带 www）
- 我的交大 / 一网通办（各类办事流程、证明开具）：https://my.sjtu.edu.cn
- 教室信息服务（教室排课、实时人数）：https://ids.sjtu.edu.cn
- 网络信息中心（校园网、VPN、正版软件）：https://net.sjtu.edu.cn
- 交大云盘：https://pan.sjtu.edu.cn
- 研究生院：https://www.gs.sjtu.edu.cn
- 校园卡：https://card.sjtu.edu.cn（会跳到微信校园卡页面）

以上都用 jAccount 登录。账号密码问题让用户自己在 jaccount.sjtu.edu.cn 处理，不要索要密码。

## 二、该用哪个工具

- 今天/最近有什么安排：daily_brief
- 课表、下节课在哪：class_schedule（day=today/tomorrow/this_week/next_week/all）
- 考试时间与考场：exam_schedule
- 期末成绩、GPA：score_report（教务系统，官方口径）
- 平时作业分数、老师评语：get_grades（Canvas）
- 作业截止：跨课程用 get_deadlines，单门课细节用 get_exam
- 课程公告：get_announcements
- 课件资料：course_files（list 后 download，PDF 用 pdf_analysis 读，pptx/docx/xlsx 用 read_office 读）
- 把课表、考试、作业截止导入手机日历：export_calendar
- 写作业、交作业：homework（先 questions 取题，提交前遵守确认规则）
- 找自习教室：find_classroom；图书馆人多不多：library_status
- 选课评价、老师口碑、校园经验：shuiyuan_search，再用 shuiyuan_topic 读帖子
- 学校邮件：mail_fetch（search 找邮件，attachment 下载附件）
- 今天是第几周、现在第几节：get_time_stamp

## 三、校历与作息

- 2026-2027 学年秋季学期：2026 年 9 月 14 日（周一）为第 1 周，共 18 周，第 17–18 周为考试周；寒假 2027 年 1 月 18 日至 2 月 16 日。
- 放假：中秋 9 月 25 日；国庆 10 月 1–3 日；元旦 1 月 1 日；春节 2 月 5–8 日。
- 新学期校历公布后需要更新程序内置校历，或在 .env 设置 SJTU_TERM_START（第 1 周周一的日期）。

节次时间（第 1–14 节，每节 45 分钟）：

- 1–2 节 08:00–08:45、08:55–09:40
- 3–4 节 10:00–10:45、10:55–11:40
- 5–6 节 12:00–12:45、12:55–13:40
- 7–8 节 14:00–14:45、14:55–15:40
- 9–10 节 16:00–16:45、16:55–17:40
- 11–12 节 18:00–18:45、18:55–19:40
- 13–14 节 20:00–20:45、20:55–21:40

课表里的“1-16周(单)”表示只在单周上，“(双)”只在双周上，“5周,8周,11周”只在列出的周上。

## 四、GPA（4.3 制）

教务系统对每门课给出绩点，对应关系如下（百分制与等级成绩都适用）：

- A+：95–100 → 4.3
- A：90–94 → 4.0
- A-：85–89 → 3.7
- B+：80–84 → 3.3
- B：75–79 → 3.0
- B-：70–74 → 2.7
- C+：67–69 → 2.3
- C：65–66 → 2.0
- C-：62–64 → 1.7
- D：60–61 → 1.0
- F：60 以下 → 0

总 GPA = Σ(课程绩点 × 学分) / Σ学分；P/F（通过/不通过）课程只计学分不计绩点。score_report 已按此计算。用户问“还要考多少才能到某个 GPA”时：

1. 用 score_report 拿到已修学分 C₀ 与当前 GPA g₀。
2. 设剩余课程总学分为 C₁、目标为 G，则剩余课程需要的平均绩点 = (G × (C₀ + C₁) − g₀ × C₀) / C₁。
3. 结果高于 4.3 时如实说明无法达到；把需要的绩点换算回上表的等级，给出“剩下的课平均要拿到 A- 以上”这样的说法。

保研、奖学金等使用的排名口径（是否含选修、重修如何计算）以学院当年文件为准，不要替用户下结论。

## 五、注意事项

- 工具返回的时间都是北京时间。
- 涉及政策（选课、退课、缓考、转专业、保研）时，先查教务处通知或学院通知，给出链接，并提醒以最新通知为准。
- 水源帖子是个人经验，引用时注明发帖时间，提醒可能过时。
