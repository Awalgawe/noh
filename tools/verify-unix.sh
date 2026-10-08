#!/usr/bin/env bash
# Native Linux/macOS qualification. Never downloads speech models or publishes.
set -euo pipefail

task_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$task_root"
task_audio=0
task_capture=1
task_transcription=0
task_offline=0
while (($#)); do
    case "$1" in
        --audio) task_audio=1 ;;
        --transcription) task_transcription=1 ;;
        --no-capture) task_capture=0 ;;
        --offline) task_offline=1 ;;
        *) printf 'Unknown option: %s\n' "$1" >&2; exit 2 ;;
    esac
    shift
done

task_os="$(uname -s)"
case "$task_os" in
    Linux|Darwin) ;;
    *) echo 'Run this script on Linux or macOS.' >&2; exit 2 ;;
esac
for task_tool in cargo rustc git; do
    command -v "$task_tool" >/dev/null || { echo "Missing tool: $task_tool" >&2; exit 2; }
done
if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
    echo 'Unset CARGO_BUILD_TARGET: this qualification must run on the native host.' >&2
    exit 2
fi
if [[ -z "${NOH_FFMPEG:-}" && "$task_os" == Darwin ]]; then
    for task_engine in /opt/homebrew/opt/ffmpeg@7/bin/ffmpeg /usr/local/opt/ffmpeg@7/bin/ffmpeg; do
        if [[ -x "$task_engine" ]]; then
            export NOH_FFMPEG="$task_engine"
            break
        fi
    done
fi
export NOH_FFMPEG="$(command -v "${NOH_FFMPEG:-ffmpeg}")"
[[ -x "$NOH_FFMPEG" ]] || { echo 'Install FFmpeg or set NOH_FFMPEG.' >&2; exit 2; }
# Normalize relative choices for the MCP adapter and child working directories.
NOH_FFMPEG="$(cd "$(dirname "$NOH_FFMPEG")" && pwd)/$(basename "$NOH_FFMPEG")"
task_filters="$("$NOH_FFMPEG" -hide_banner -filters 2>/dev/null)"
if ! awk '$2 == "subtitles" { found=1 } END { exit !found }' <<< "$task_filters"; then
    echo 'FFmpeg needs libass (subtitles filter). On macOS: brew install ffmpeg@7.' >&2
    exit 2
fi
if ((task_transcription)); then
    for task_key in NOH_WHISPER NOH_WHISPER_MODEL NOH_WHISPER_VAD NOH_SUBTITLE_SPEECH_FR; do
        [[ -f "${!task_key:-}" ]] || { echo "Set $task_key to an existing native resource." >&2; exit 2; }
    done
fi
if ((task_capture)) && [[ "$task_os" == Linux && -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    command -v xvfb-run >/dev/null || { echo 'GUI capture needs a display or xvfb-run.' >&2; exit 2; }
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$task_root/target}"
mkdir -p "$CARGO_TARGET_DIR"
CARGO_TARGET_DIR="$(cd "$CARGO_TARGET_DIR" && pwd)"
task_evidence="$task_root/logs/platform/$(date -u +%Y%m%dT%H%M%SZ)-$$"
mkdir -p "$task_evidence"
exec > >(tee "$task_evidence/run.log") 2>&1
trap 'task_exit=$?; printf "Exit: %s\nEvidence: %s\n" "$task_exit" "$task_evidence"; exit "$task_exit"' EXIT

uname -a
rustc -Vv
cargo -V
"$NOH_FFMPEG" -version
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git rev-parse HEAD
    git status --short
fi
printf 'Physical audio: %s; speech recognition: %s; GUI captures: %s\n' \
    "$task_audio" "$task_transcription" "$task_capture"
# Always qualify the binaries from this checkout, even in a configured shell.
unset NOH_EXE NOH_APP_EXE NOH_MCP_EXE
export NOH_TEST_PLAYBACK_AUDIO="$task_audio"

cargo fmt --check
# The GUI unit tests launch the normal desktop worker beside their test binary.
task_build=(build --locked --features gui,mcp,updates --bins --example dev)
if ((task_offline)); then task_build+=(--offline); fi
cargo "${task_build[@]}"

task_dev="$CARGO_TARGET_DIR/debug/examples/dev"
task_app="$CARGO_TARGET_DIR/debug/noh-app"
task_verify=(verify --media --mcp --updates)
if ((task_offline)); then task_verify+=(--offline); fi
if ((task_transcription)); then task_verify+=(--transcription); fi
"$task_dev" "${task_verify[@]}"
"$task_app" --build-info > "$task_evidence/build-info.json"

if ((task_capture)); then
    "$task_dev" fixtures layout "$task_evidence/fixtures" --ffmpeg "$NOH_FFMPEG"
    task_gui=("$task_dev" capture-ui --app "$task_app" --ffmpeg "$NOH_FFMPEG"
        --output "$task_evidence/captures" --project "$task_evidence/fixtures/project.json"
        --states ready --languages en,ja --themes dark --scales 1 --sizes 980x850,420x540)
    if [[ "$task_os" == Linux && -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
        xvfb-run -a -s '-screen 0 1280x1024x24' "${task_gui[@]}"
    else
        "${task_gui[@]}"
    fi
fi
echo 'Requested native checks passed. Review the captures before accepting GUI layout.'
if ((!task_audio)); then echo 'Physical audio was not qualified; rerun with --audio on a desktop.'; fi
if ((!task_transcription)); then echo 'Real Whisper recognition/GPU was not qualified; use --transcription with native resources.'; fi
