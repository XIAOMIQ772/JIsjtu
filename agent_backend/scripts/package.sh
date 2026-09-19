#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
frontend_dir=${AGENT_PACKAGE_FRONTEND_DIR:-"$repo_dir/../agent_frontend"}
for asset in index.html style.css app.js; do
    [ -f "$frontend_dir/$asset" ] || { printf '缺少前端文件：%s\n' "$frontend_dir/$asset" >&2; exit 1; }
done
command -v zip >/dev/null 2>&1 || { printf '请先安装 zip。\n' >&2; exit 1; }
host=$(rustc -vV | sed -n 's/^host: //p')
package_os=$(uname -s)
package_arch=$(uname -m)
case "$package_os" in Darwin|Linux) ;; *) printf '当前安装脚本支持 macOS / Linux。\n' >&2; exit 1 ;; esac

# Explicit native target keeps an unrelated Cargo default target from leaking in.
cargo build --manifest-path "$repo_dir/Cargo.toml" --locked --release --target "$host" --target-dir "$repo_dir/target"
mkdir -p "$repo_dir/dist" "$repo_dir/target/packages"
stage=$(mktemp -d "$repo_dir/target/packages/build.XXXXXX")
package_name="JIsjtu-$host"
package_dir="$stage/$package_name"
mkdir -p "$package_dir/agent_frontend"
cp "$repo_dir/target/$host/release/agent" "$package_dir/agent"
chmod 755 "$package_dir/agent"
for asset in index.html style.css app.js; do
    cp "$frontend_dir/$asset" "$package_dir/agent_frontend/$asset"
done
sed -e "s/@PACKAGE_OS@/$package_os/g" -e "s/@PACKAGE_ARCH@/$package_arch/g" \
    "$repo_dir/packaging/install.sh" > "$package_dir/install.sh"
chmod 755 "$package_dir/install.sh"
(cd "$stage" && zip -q -r "$package_name.zip" "$package_name")
mv -f "$stage/$package_name.zip" "$repo_dir/dist/$package_name.zip"
printf '\n安装包：%s/dist/%s.zip\n' "$repo_dir" "$package_name"
printf '仅包含安装脚本、后端程序和前端目录，不包含个人 .env。\n'
