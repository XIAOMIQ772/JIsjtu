#!/bin/sh
set -eu

package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
config="$HOME/.local/share/JIsjtu/.env"
if [ "$#" -eq 0 ] && [ -t 0 ] && [ ! -e "$config" ]; then
    /bin/sh "$package_dir/install.sh"
    printf '\n请在「文本编辑」中填写模型配置，按 ⌘S 保存，然后打开新终端输入 JIsjtu。\n'
    /usr/bin/open -e "$config" || printf '可在终端运行 open -e ~/.local/share/JIsjtu/.env 打开配置文件。\n' >&2
else
    exec /bin/sh "$package_dir/install.sh" "$@"
fi
