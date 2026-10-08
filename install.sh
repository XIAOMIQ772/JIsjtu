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
package_os='Darwin'
package_arch='arm64'
package_target='aarch64-apple-darwin'
[ "$(uname -s)" = "$package_os" ] || fail "此安装包适用于 $package_os，请下载当前系统对应的版本"
[ "$(uname -m)" = "$package_arch" ] || fail "此安装包适用于 $package_arch，请下载当前处理器对应的版本"

# The same rendered installer works in the ZIP and at the repository root.
agent_asset="$package_dir/agent"
config_asset="$package_dir/.env.example"
requirements_asset="$package_dir/requirements-sso.txt"
readme_asset="$package_dir/README.md"
if [ ! -f "$agent_asset" ] && [ -f "$package_dir/agent_backend/Cargo.toml" ]; then
    backend_dir="$package_dir/agent_backend"
    for candidate in "$backend_dir/target/$package_target/release/agent" "$backend_dir/target/release/agent"; do
        if [ -f "$candidate" ] && { [ ! -f "$agent_asset" ] || [ "$candidate" -nt "$agent_asset" ]; }; then
            agent_asset=$candidate
        fi
    done
    config_asset="$backend_dir/.env.example"
    requirements_asset="$backend_dir/requirements-sso.txt"
    readme_asset="$backend_dir/packaging/README.md"
fi
[ -f "$agent_asset" ] || fail '缺少 release 程序，请先构建 release 或使用完整安装包'
for asset in agent_frontend/index.html agent_frontend/style.css agent_frontend/app.js LICENSE; do
    [ -f "$package_dir/$asset" ] || fail "安装包缺少 $asset"
done
[ -d "$package_dir/skills" ] || fail '安装包缺少 skills 目录'
[ -f "$config_asset" ] || fail '安装包缺少 .env.example'
[ -f "$requirements_asset" ] || fail '安装包缺少 requirements-sso.txt'
[ -f "$readme_asset" ] || fail '安装包缺少 README.md'

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
mkdir -p "$app_dir/agent_frontend" "$app_dir/skills" "$bin_dir"
printf 'JIsjtu\n' > "$app_dir/.jisjtu-install"

# Stage the executable before replacing it, so updates also work on Linux.
binary_stage=$(mktemp "$app_dir/.agent.XXXXXX")
cp "$agent_asset" "$binary_stage"
chmod 755 "$binary_stage"
mv -f "$binary_stage" "$app_dir/agent"
for asset in index.html style.css app.js; do
    cp "$package_dir/agent_frontend/$asset" "$app_dir/agent_frontend/$asset"
done
# 合并而非替换：升级时保留用户自己添加的技能
cp -R "$package_dir/skills/." "$app_dir/skills/"
cp "$config_asset" "$app_dir/.env.example"
cp "$requirements_asset" "$app_dir/requirements-sso.txt"
cp "$readme_asset" "$app_dir/README.md"
cp "$package_dir/LICENSE" "$app_dir/LICENSE"

if [ ! -e "$app_dir/.env" ]; then
    cp "$config_asset" "$app_dir/.env"
    chmod 600 "$app_dir/.env"
    printf '已生成配置模板：%s/.env\n' "$app_dir"
else
    printf '已保留原配置：%s/.env\n' "$app_dir"
    printf '升级提示：请检查原配置是否包含非空的 MODEL；缺少时请手动补上接口支持的模型名称。\n'
fi

launcher_stage=$(mktemp "$bin_dir/.JIsjtu.XXXXXX")
cat > "$launcher_stage" <<'LAUNCHER'
#!/bin/sh
# Managed by JIsjtu installer
set -eu
app_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../share/JIsjtu" && pwd)
if [ "$#" -eq 0 ]; then
    set -- --tui
fi
if [ "$#" -gt 0 ]; then
    case "$1" in
        --config) printf '%s/.env\n' "$app_dir"; exit 0 ;;
        --help) printf 'JIsjtu             默认启动终端界面（TUI）\nJIsjtu --web       启动网页界面，也可使用 JIsjtu web\nJIsjtu --tui       显式启动终端界面\nJIsjtu --sso-login 单独检查或登录 jAccount\nJIsjtu --config    显示配置文件位置\n按 Ctrl+C 停止服务。\n'; exit 0 ;;
        --web|web) shift ;;
        --sso-login|--tui) ;;
        *) printf '未知参数：%s；使用 JIsjtu --help 查看帮助。\n' "$1" >&2; exit 1 ;;
    esac
fi
cd -- "$app_dir"
exec ./agent "$@"
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
printf '对话必填：OPENAI_API_KEY、OPENAI_BASE_URL、MODEL。\n'
printf '新开终端后运行：JIsjtu\n当前终端立即使用可先执行：\n%s\n' "$path_line"
printf '默认进入终端界面；需要网页界面请运行：JIsjtu --web\n'
printf '查看配置位置：JIsjtu --config\n'
printf '技能目录：%s/skills，放 <名字>/SKILL.md 即可扩展，改完重启生效。\n' "$app_dir"
printf '浏览器工具还需 Python 3.10+ 和 Edge / Chrome；SSO 依赖安装见：%s/README.md\n' "$app_dir"
printf '升级后请重新启动正在运行的 JIsjtu。\n'
printf '安装文件已复制，解压目录可以删除。\n'
