#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'EOF'
用法：check-linux-glibc.sh --max <版本> --binary-name <程序名> <产物>...

从 DEB、RPM 或 AppImage 最终产物中提取主程序，并检查其最高 GLIBC
版本要求是否超过允许上限。
EOF
}

die() {
  FAILURE_REASON="$*"
  echo "错误：$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "缺少必要命令：$1"
}

is_version() {
  [[ "$1" =~ ^[0-9]+([.][0-9]+)+$ ]]
}

version_greater_than() {
  local left="$1"
  local right="$2"
  local left_part right_part index length
  local -a left_parts right_parts
  local IFS='.'
  read -r -a left_parts <<<"$left"
  read -r -a right_parts <<<"$right"

  length=${#left_parts[@]}
  if (( ${#right_parts[@]} > length )); then
    length=${#right_parts[@]}
  fi

  for ((index = 0; index < length; index++)); do
    left_part=${left_parts[index]:-0}
    right_part=${right_parts[index]:-0}
    if ((10#$left_part > 10#$right_part)); then
      return 0
    fi
    if ((10#$left_part < 10#$right_part)); then
      return 1
    fi
  done

  return 1
}

MAX_GLIBC_VERSION=""
BINARY_NAME=""
ARTIFACTS=()

while (($# > 0)); do
  case "$1" in
    --max)
      (($# >= 2)) || die "--max 缺少版本参数"
      MAX_GLIBC_VERSION="$2"
      shift 2
      ;;
    --binary-name)
      (($# >= 2)) || die "--binary-name 缺少程序名"
      BINARY_NAME="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --*)
      die "未知参数：$1"
      ;;
    *)
      ARTIFACTS+=("$1")
      shift
      ;;
  esac
done

[[ -n "$MAX_GLIBC_VERSION" ]] || die "必须显式提供 --max"
[[ -n "$BINARY_NAME" ]] || die "必须显式提供 --binary-name"
is_version "$MAX_GLIBC_VERSION" || die "--max 必须是数字版本，例如 2.35"
((${#ARTIFACTS[@]} > 0)) || die "至少需要提供一个 Linux 产物"

require_command readelf

RESULT_LINES=()
FAILED=0
FAILURE_REASON=""
CURRENT_ARTIFACT="Linux GLIBC 检查"
TEMP_ROOT="$(mktemp -d)"

finish() {
  local status=$?
  trap - EXIT
  set +e

  if ((status != 0)) && [[ -n "$FAILURE_REASON" ]]; then
    RESULT_LINES+=("- ❌ ${CURRENT_ARTIFACT}: ${FAILURE_REASON}")
  fi

  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
      echo "### Linux GLIBC 兼容性"
      echo
      printf '%s\n' "${RESULT_LINES[@]}"
    } >>"$GITHUB_STEP_SUMMARY" || true
  fi

  rm -rf "$TEMP_ROOT"
  exit "$status"
}
trap finish EXIT

extract_artifact() {
  local artifact="$1"
  local output_dir="$2"

  mkdir -p "$output_dir"
  case "$artifact" in
    *.deb)
      require_command dpkg-deb
      dpkg-deb -x "$artifact" "$output_dir" || die "DEB 解包失败：$artifact"
      BINARY_PATH="$output_dir/usr/bin/$BINARY_NAME"
      ;;
    *.rpm)
      require_command rpm2cpio
      require_command cpio
      if ! (
        cd "$output_dir"
        rpm2cpio "$artifact" | cpio -idm --quiet
      ); then
        die "RPM 解包失败：$artifact"
      fi
      BINARY_PATH="$output_dir/usr/bin/$BINARY_NAME"
      ;;
    *.AppImage)
      chmod +x "$artifact" || die "AppImage 无法设置执行权限：$artifact"
      if ! (
        cd "$output_dir"
        "$artifact" --appimage-extract >/dev/null
      ); then
        die "AppImage 解包失败：$artifact"
      fi
      BINARY_PATH="$output_dir/squashfs-root/usr/bin/$BINARY_NAME"
      ;;
    *)
      die "不支持的 Linux 产物格式：$artifact"
      ;;
  esac
}

highest_glibc_version() {
  local binary="$1"
  local version highest=""
  local version_info

  readelf --file-header "$binary" >/dev/null 2>&1 || die "主程序不是有效 ELF：$binary"
  version_info="$(readelf --version-info --wide "$binary")" || die "无法读取 ELF 版本信息：$binary"

  while IFS= read -r version; do
    version=${version#GLIBC_}
    is_version "$version" || continue
    if [[ -z "$highest" ]] || version_greater_than "$version" "$highest"; then
      highest="$version"
    fi
  done < <(printf '%s\n' "$version_info" | grep -oE 'GLIBC_[0-9]+([.][0-9]+)+' | sort -u || true)

  [[ -n "$highest" ]] || die "主程序中未发现 GLIBC 版本要求：$binary"
  HIGHEST_GLIBC_VERSION="$highest"
}

for index in "${!ARTIFACTS[@]}"; do
  artifact="${ARTIFACTS[index]}"
  CURRENT_ARTIFACT="$(basename -- "$artifact")"
  FAILURE_REASON=""
  [[ -f "$artifact" ]] || die "产物不存在：$artifact"
  artifact_dir="$(cd -- "$(dirname -- "$artifact")" && pwd -P)"
  artifact="$artifact_dir/$(basename -- "$artifact")"

  extract_dir="$TEMP_ROOT/artifact-$index"
  BINARY_PATH=""
  extract_artifact "$artifact" "$extract_dir"
  binary="$BINARY_PATH"
  [[ -f "$binary" ]] || die "产物内找不到主程序 usr/bin/${BINARY_NAME}：$artifact"

  HIGHEST_GLIBC_VERSION=""
  highest_glibc_version "$binary"
  highest="$HIGHEST_GLIBC_VERSION"
  result="$(basename "$artifact"): GLIBC_${highest}（上限 GLIBC_${MAX_GLIBC_VERSION}）"
  echo "$result"

  if version_greater_than "$highest" "$MAX_GLIBC_VERSION"; then
    echo "错误：$(basename "$artifact") 要求 GLIBC_${highest}，超过允许上限 GLIBC_${MAX_GLIBC_VERSION}" >&2
    RESULT_LINES+=("- ❌ $result")
    FAILED=1
  else
    RESULT_LINES+=("- ✅ $result")
  fi
done

((FAILED == 0)) || exit 1
