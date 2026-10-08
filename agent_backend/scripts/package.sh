#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
project_dir=$(CDPATH= cd -- "$repo_dir/.." && pwd)
frontend_dir=${AGENT_PACKAGE_FRONTEND_DIR:-"$repo_dir/../agent_frontend"}
skills_dir=${AGENT_PACKAGE_SKILLS_DIR:-"$repo_dir/../skills"}
for asset in index.html style.css app.js; do
    [ -f "$frontend_dir/$asset" ] || { printf '缺少前端文件：%s\n' "$frontend_dir/$asset" >&2; exit 1; }
done
[ -d "$skills_dir" ] || { printf '缺少技能目录：%s\n' "$skills_dir" >&2; exit 1; }
for asset in .env.example requirements-sso.txt packaging/README.md packaging/install.sh; do
    [ -f "$repo_dir/$asset" ] || { printf '缺少安装文件：%s\n' "$repo_dir/$asset" >&2; exit 1; }
done
[ -f "$project_dir/LICENSE" ] || { printf '缺少 LICENSE。\n' >&2; exit 1; }
command -v zip >/dev/null 2>&1 || { printf '请先安装 zip。\n' >&2; exit 1; }
host=$(rustc -vV | sed -n 's/^host: //p')
package_os=$(uname -s)
package_arch=$(uname -m)
case "$package_os" in Darwin|Linux) ;; *) printf '当前安装脚本支持 macOS / Linux。\n' >&2; exit 1 ;; esac

# An existing, tested native release can be packaged without rebuilding it.
if [ -n "${AGENT_PACKAGE_BINARY:-}" ]; then
    binary=$AGENT_PACKAGE_BINARY
    [ -f "$binary" ] || { printf 'release 程序不存在：%s\n' "$binary" >&2; exit 1; }
else
    # Explicit native target prevents an unrelated Cargo default target leaking in.
    cargo build --manifest-path "$repo_dir/Cargo.toml" --locked --release --target "$host" --target-dir "$repo_dir/target"
    binary="$repo_dir/target/$host/release/agent"
fi
if [ "$package_os" = Darwin ]; then
    lipo -verify_arch "$package_arch" "$binary" || { printf 'release 程序与当前 Mac 架构不匹配。\n' >&2; exit 1; }
fi
mkdir -p "$repo_dir/dist" "$repo_dir/target/packages"
stage=$(mktemp -d "$repo_dir/target/packages/build.XXXXXX")
trap 'rm -rf "$stage"' EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
package_name="JIsjtu-$host"
package_dir="$stage/$package_name"
mkdir -p "$package_dir/agent_frontend" "$package_dir/skills"
cp "$binary" "$package_dir/agent"
chmod 755 "$package_dir/agent"
for asset in index.html style.css app.js; do
    cp "$frontend_dir/$asset" "$package_dir/agent_frontend/$asset"
done
cp -R "$skills_dir/." "$package_dir/skills/"
cp "$repo_dir/.env.example" "$package_dir/.env.example"
cp "$repo_dir/requirements-sso.txt" "$package_dir/requirements-sso.txt"
cp "$repo_dir/packaging/README.md" "$package_dir/README.md"
cp "$project_dir/LICENSE" "$package_dir/LICENSE"
sed -e "s/@PACKAGE_OS@/$package_os/g" -e "s/@PACKAGE_ARCH@/$package_arch/g" -e "s/@PACKAGE_TARGET@/$host/g" \
    "$repo_dir/packaging/install.sh" > "$package_dir/install.sh"
chmod 755 "$package_dir/install.sh"
(cd "$stage" && zip -X -q -r "$package_name.zip" "$package_name")
(cd "$package_dir" && zip -X -q -r "$stage/JIsjtu.zip" .)
mv -f "$stage/$package_name.zip" "$repo_dir/dist/$package_name.zip"
mv -f "$stage/JIsjtu.zip" "$project_dir/JIsjtu.zip"
cp "$package_dir/install.sh" "$project_dir/install.sh"
printf '\n安装包：%s/dist/%s.zip\n' "$repo_dir" "$package_name"
printf '已同步：%s/JIsjtu.zip 和 install.sh\n' "$project_dir"
printf '包含程序、前端、技能、安装说明、配置示例与 SSO 依赖清单，不包含个人配置或会话。\n'
