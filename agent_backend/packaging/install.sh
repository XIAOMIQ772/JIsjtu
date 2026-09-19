#!/bin/sh
# JIsjtu installer: no Rust, Node, or root privileges required.
set -eu
umask 077

fail() { printf '安装失败：%s\n' "$*" >&2; exit 1; }
package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix="${HOME}/.local"
register_path=yes
profile_override=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix) [ "$#" -ge 2 ] || fail '--prefix 缺少目录'; prefix=$2; shift 2 ;;
        --profile) [ "$#" -ge 2 ] || fail '--profile 缺少文件'; profile_override=$2; shift 2 ;;
        --no-path) register_path=no; shift ;;
        --help) printf '用法：sh install.sh [--prefix 安装根目录] [--no-path] [--profile shell配置文件]\n'; exit 0 ;;
        *) fail "未知参数：$1" ;;
    esac
done
case "$prefix" in /*) ;; *) fail '--prefix 必须是绝对路径' ;; esac
case "$prefix" in *'
'*) fail '安装路径不能包含换行符' ;; esac

# Filled by the packaging script for the actual build target.
package_os='@PACKAGE_OS@'
package_arch='@PACKAGE_ARCH@'
[ "$(uname -s)" = "$package_os" ] || fail "此安装包适用于 $package_os，请下载当前系统对应的版本"
[ "$(uname -m)" = "$package_arch" ] || fail "此安装包适用于 $package_arch，请下载当前处理器对应的版本"
for asset in agent agent_frontend/index.html agent_frontend/style.css agent_frontend/app.js; do
    [ -f "$package_dir/$asset" ] || fail "安装包缺少 $asset"
done

mkdir -p "$prefix"
prefix=$(CDPATH= cd -- "$prefix" && pwd)
app_dir="$prefix/share/JIsjtu"
bin_dir="$prefix/bin"
launcher="$bin_dir/JIsjtu"
if [ -e "$app_dir" ] || [ -L "$app_dir" ]; then
    [ ! -L "$app_dir" ] && [ -f "$app_dir/.jisjtu-install" ] || fail "$app_dir 已存在且不属于本安装器，请选择其他 --prefix"
fi
if [ -e "$launcher" ] || [ -L "$launcher" ]; then
    [ ! -L "$launcher" ] && grep -Fqx '# Managed by JIsjtu installer' "$launcher" || fail "$launcher 已存在且不属于本安装器"
fi
mkdir -p "$app_dir/agent_frontend" "$bin_dir"
printf 'JIsjtu\n' > "$app_dir/.jisjtu-install"

# Stage the executable before replacing it, so updates also work on Linux.
binary_stage=$(mktemp "$app_dir/.agent.XXXXXX")
cp "$package_dir/agent" "$binary_stage"
chmod 755 "$binary_stage"
mv -f "$binary_stage" "$app_dir/agent"
for asset in index.html style.css app.js; do
    cp "$package_dir/agent_frontend/$asset" "$app_dir/agent_frontend/$asset"
done

if [ ! -e "$app_dir/.env" ]; then
    cat > "$app_dir/.env" <<'CONFIG'
# 必填：模型服务的 API 密钥及兼容 OpenAI 的接口地址。
# 当前后端使用 deepseek-flash，接口必须支持该模型。
OPENAI_API_KEY=
OPENAI_BASE_URL=

# Canvas 课程、作业、课件查询需要此令牌。
CANVAS_API_TOKEN=

# 校园邮箱功能需要以下配置。
EMAIL_USER_ACCOUNT=
EMAIL_USER_PASSWORD=
IMAP_HOST=
IMAP_PORT=993

AGENT_HTTP_PORT=8080
AGENT_TCP_PORT=8081
CONFIG
    chmod 600 "$app_dir/.env"
    printf '已生成配置模板：%s/.env\n' "$app_dir"
else
    printf '已保留原配置：%s/.env\n' "$app_dir"
fi

launcher_stage=$(mktemp "$bin_dir/.JIsjtu.XXXXXX")
cat > "$launcher_stage" <<'LAUNCHER'
#!/bin/sh
# Managed by JIsjtu installer
set -eu
app_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../share/JIsjtu" && pwd)
if [ "$#" -gt 0 ]; then
    case "$1" in
        --config) printf '%s/.env\n' "$app_dir"; exit 0 ;;
        --help) printf 'JIsjtu          启动服务并打开网页\nJIsjtu --config 显示配置文件位置\n按 Ctrl+C 停止服务。\n'; exit 0 ;;
        *) printf '未知参数：%s；使用 JIsjtu --help 查看帮助。\n' "$1" >&2; exit 1 ;;
    esac
fi
cd -- "$app_dir"
exec ./agent
LAUNCHER
chmod 755 "$launcher_stage"
mv -f "$launcher_stage" "$launcher"

# Quote the entire path for shell startup files (including spaces and quotes).
quoted_bin=$(printf '%s' "$bin_dir" | sed "s/'/'\\\\''/g")
path_line="export PATH='$quoted_bin':\"\$PATH\""
add_profile() {
    profile=$1
    mkdir -p "$(dirname -- "$profile")"
    if [ ! -f "$profile" ] || ! grep -Fqx "$path_line" "$profile"; then
        printf '\n# JIsjtu command path\n%s\n' "$path_line" >> "$profile"
    fi
    printf '已注册命令路径：%s\n' "$profile"
}
if [ "$register_path" = yes ]; then
    if [ -n "$profile_override" ]; then
        add_profile "$profile_override"
    else
        case "${SHELL:-}" in
            */zsh) add_profile "${ZDOTDIR:-$HOME}/.zshrc" ;;
            */bash)
                add_profile "$HOME/.bashrc"
                if [ -f "$HOME/.bash_profile" ]; then add_profile "$HOME/.bash_profile"
                elif [ -f "$HOME/.bash_login" ]; then add_profile "$HOME/.bash_login"
                else add_profile "$HOME/.profile"; fi ;;
            *) add_profile "$HOME/.profile" ;;
        esac
    fi
fi
printf '\n安装完成。请先填写：%s/.env\n' "$app_dir"
printf '新开终端后运行：JIsjtu\n当前终端立即使用可先执行：\n%s\n' "$path_line"
printf '查看配置位置：JIsjtu --config\n安装文件已复制，解压目录可以删除。\n'
