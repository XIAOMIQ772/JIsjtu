"""Send one plain-text campus email. Read its envelope/content from stdin and return a JSON receipt."""
from email.headerregistry import Address
from email.message import EmailMessage
from email.utils import formatdate, make_msgid
import json
import os
import smtplib
import ssl
import sys


def send(draft):
    account = os.environ.get("EMAIL_USER_ACCOUNT", "").strip()
    password = (os.environ.get("SMTP_PASSWORD") or os.environ.get("IMAP_PASSWORD")
                or os.environ.get("EMAIL_USER_PASSWORD"))
    if not account or not password:
        return {"error": "请配置 EMAIL_USER_ACCOUNT，以及 SMTP_PASSWORD、IMAP_PASSWORD 或 EMAIL_USER_PASSWORD。"}
    try:
        sender = Address(addr_spec=account if "@" in account else account + "@sjtu.edu.cn")
        recipient = Address(addr_spec=draft["to"])
        host = os.environ.get("SMTP_HOST") or "mail.sjtu.edu.cn"
        port = int(os.environ.get("SMTP_PORT") or 465)
        if not 1 <= port <= 65535:
            raise ValueError("port")
        message = EmailMessage()
        message["From"] = sender
        message["To"] = recipient
        message["Subject"] = draft["subject"]
        message["Date"] = formatdate(localtime=True)
        message["Message-ID"] = make_msgid(domain=sender.domain)
        message.set_content(draft["body"], charset="utf-8")
    except ValueError:
        return {"error": "请检查发件地址、收件地址及 SMTP_PORT 配置。"}

    smtp = None
    sending = False
    try:
        context = ssl.create_default_context()
        if port == 587:
            smtp = smtplib.SMTP(host, port, timeout=25)
            smtp.starttls(context=context)
        else:
            smtp = smtplib.SMTP_SSL(host, port, context=context, timeout=25)
        smtp.login(sender.addr_spec, password)
        sending = True
        smtp.send_message(message, from_addr=sender.addr_spec, to_addrs=[recipient.addr_spec])
        return {"accepted": True}
    except smtplib.SMTPAuthenticationError:
        return {"error": "SMTP 登录失败，请检查邮箱客户端授权码。"}
    except smtplib.SMTPRecipientsRefused:
        return {"error": "SMTP 服务器拒绝了收件地址，邮件未被接收。"}
    except smtplib.SMTPResponseException as error:
        return {"error": f"SMTP 服务器拒绝了请求（{error.smtp_code}）。"}
    except (smtplib.SMTPException, OSError):
        if sending:
            return {"error": "SMTP 投递期间连接中断，结果未知。请先向收件人或邮箱管理员核实，再决定是否重发。"}
        return {"error": "未能建立通过验证的 SMTP 连接，请检查 SMTP_HOST、端口和网络。"}
    finally:
        if smtp is not None:
            # DATA has its own acknowledgement. Losing QUIT must never turn accepted mail into a retry.
            try:
                smtp.quit()
            except (smtplib.SMTPException, OSError):
                smtp.close()


if __name__ == "__main__":
    try:
        result = send(json.loads(sys.stdin.buffer.read().decode("utf-8")))
    except Exception:
        result = {"error": "未取得有效邮件回执，请先核实发送状态。"}
    sys.stdout.buffer.write(json.dumps(result, ensure_ascii=False).encode("utf-8"))
