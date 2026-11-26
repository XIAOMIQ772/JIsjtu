"""Open the student's Canvas-issued LTI launch without starting a browser."""
import http.cookiejar
import json
import sys
import urllib.parse
import urllib.request
from html.parser import HTMLParser

SCHOOL = "https://mlearning.sjtu.edu.cn"
LOGIN = SCHOOL + "/lms-auth-sjtu/lti/redirectRollcallRequest"


class LaunchForm(HTMLParser):
    def __init__(self):
        super().__init__()
        self.inside = False
        self.fields = {}

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "form":
            self.inside = attrs.get("action") == LOGIN and attrs.get("method", "").lower() == "post"
        elif self.inside and tag == "input" and attrs.get("type", "").lower() == "hidden" and attrs.get("name"):
            self.fields[attrs["name"]] = attrs.get("value", "")

    def handle_endtag(self, tag):
        if tag == "form":
            self.inside = False


class SchoolRedirects(urllib.request.HTTPRedirectHandler):
    def __init__(self, school_login=False):
        self.school_login = school_login

    def redirect_request(self, req, fp, code, msg, headers, url):
        # A signed Canvas launch must never be redirected. After the LTI POST,
        # follow only the school's own redirects to its student roll-call view.
        target = urllib.parse.urlsplit(url)
        if not self.school_login or target.scheme != "https" or target.netloc != "mlearning.sjtu.edu.cn":
            raise ValueError("unexpected redirect")
        return super().redirect_request(req, fp, code, msg, headers, url)


def exchange(course_id, signed_url):
    launch = urllib.parse.urlsplit(signed_url)
    if (launch.scheme != "https" or launch.netloc != "oc.sjtu.edu.cn"
            or launch.path != f"/courses/{course_id}/external_tools/sessionless_launch"):
        raise ValueError("unexpected Canvas URL")
    without_redirects = urllib.request.build_opener(SchoolRedirects())
    with without_redirects.open(signed_url, timeout=8) as response:
        html = response.read(512 * 1024 + 1)
    if len(html) > 512 * 1024:
        raise ValueError("unexpected Canvas response")
    form = LaunchForm()
    form.feed(html.decode("utf-8"))
    if not form.fields.get("oauth_signature"):
        raise ValueError("missing signed form")
    cookies = http.cookiejar.CookieJar()
    school = urllib.request.build_opener(SchoolRedirects(True), urllib.request.HTTPCookieProcessor(cookies))
    payload = urllib.parse.urlencode(form.fields).encode("ascii")
    with school.open(LOGIN, payload, timeout=8) as response:
        landing = urllib.parse.urlsplit(response.url)
    params = urllib.parse.parse_qs(landing.query)
    if (landing.scheme != "https" or landing.netloc != "mlearning.sjtu.edu.cn"
            or landing.path.rstrip("/") != "/lms/rollcall" or params.get("roleCode") != ["StudentEnrollment"]):
        raise ValueError("not the student view")
    # The official student page copies its token cookie to the Authorization
    # header for its own records endpoint. Never export other school cookies.
    token = next(c.value for c in cookies if c.name == "token" and c.domain.lstrip(".") == landing.hostname)
    code, = params["courseCode"]
    if not code or not token:
        raise ValueError("incomplete student session")
    return {"course_code": code, "token": token}


if __name__ == "__main__":
    try:
        data = json.load(sys.stdin)
        json.dump(exchange(int(data["course_id"]), data["url"]), sys.stdout)
    except Exception:
        # Launch URLs, form bodies and HTTP exceptions can contain credentials.
        sys.exit(1)
