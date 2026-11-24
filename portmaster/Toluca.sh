#!/bin/bash

XDG_DATA_HOME=${XDG_DATA_HOME:-$HOME/.local/share}

if [ -d "/opt/system/Tools/PortMaster/" ]; then
  controlfolder="/opt/system/Tools/PortMaster"
elif [ -d "/opt/tools/PortMaster/" ]; then
  controlfolder="/opt/tools/PortMaster"
elif [ -d "$XDG_DATA_HOME/PortMaster/" ]; then
  controlfolder="$XDG_DATA_HOME/PortMaster"
else
  controlfolder="/roms/ports/PortMaster"
fi

# shellcheck disable=SC1091
source "$controlfolder/control.txt"
# shellcheck disable=SC1090,SC1091,SC2154
[ -f "${controlfolder}/mod_${CFW_NAME}.txt" ] && source "${controlfolder}/mod_${CFW_NAME}.txt"
get_controls

# shellcheck disable=SC2154
GAMEDIR="/$directory/ports/toluca"
cd "$GAMEDIR" || exit 1

true > "$GAMEDIR/log.txt" && exec > >(tee "$GAMEDIR/log.txt") 2>&1

if [ -f "$GAMEDIR/toluca.conf" ]; then
  # shellcheck disable=SC1091
  source "$GAMEDIR/toluca.conf"
fi

EXE_PATH=""
for candidate in \
  "$GAMEDIR/gamedata/sh2pc.exe" \
  "$GAMEDIR/gamedata/Silent Hill 2/sh2pc.exe"; do
  if [ -f "$candidate" ]; then
    EXE_PATH="$candidate"
    break
  fi
done

if [ -z "$EXE_PATH" ]; then
  echo "Error: sh2pc.exe not found in $GAMEDIR/gamedata/."
  echo "Copy vanilla North American v1.0 sh2pc.exe and data/ into $GAMEDIR/gamedata/."
  pm_finish
  exit 1
fi

GAME_DATA_DIR="$(dirname "$EXE_PATH")"
if [ ! -d "$GAME_DATA_DIR/data" ] && [ ! -d "$GAME_DATA_DIR/DATA" ] && [ ! -d "$GAME_DATA_DIR/Data" ]; then
  echo "Error: data/ directory not found in $GAME_DATA_DIR/."
  pm_finish
  exit 1
fi

CACHE_BIN="$GAMEDIR/cache/toluca_code.bin"
SHA_STAMP="$GAMEDIR/cache/sh2pc.sha1"

if command -v sha1sum >/dev/null 2>&1; then
  CURRENT_SHA1="$(sha1sum "$EXE_PATH" | awk '{ print $1 }')"
else
  CURRENT_SHA1="$("$GAMEDIR/bin/toluca-recomp" verify "$EXE_PATH" | awk '/^SHA-1:/ { print $2; exit }')"
fi

SAVED_SHA1=""
if [ -f "$SHA_STAMP" ]; then
  SAVED_SHA1="$(cat "$SHA_STAMP" 2>/dev/null || true)"
fi

if [ ! -f "$CACHE_BIN" ] || [ -z "$CURRENT_SHA1" ] || [ "$CURRENT_SHA1" != "$SAVED_SHA1" ]; then
  export PATCHER_FILE="$GAMEDIR/tools/patchscript"
  export PATCHER_GAME="Toluca"
  export PATCHER_TIME="15 to 45 seconds"
  export GAMEDIR

  if [ -f "$controlfolder/utils/patcher.txt" ]; then
    # shellcheck disable=SC1091
    source "$controlfolder/utils/patcher.txt"
  else
    bash "$PATCHER_FILE"
  fi
fi

if [ ! -f "$CACHE_BIN" ]; then
  echo "Error: AOT cache generation failed ($CACHE_BIN missing)."
  pm_finish
  exit 1
fi

WIDTH="${TOLUCA_WIDTH:-${DISPLAY_WIDTH:-640}}"
HEIGHT="${TOLUCA_HEIGHT:-${DISPLAY_HEIGHT:-480}}"
ASPECT="${TOLUCA_ASPECT:-4:3}"

# shellcheck disable=SC2154
export SDL_GAMECONTROLLERCONFIG="$sdl_controllerconfig"

if [ -n "${DEVICE_ARCH:-}" ] && [ -d "$GAMEDIR/libs.${DEVICE_ARCH}" ]; then
  export LD_LIBRARY_PATH="$GAMEDIR/libs.${DEVICE_ARCH}:${LD_LIBRARY_PATH:-}"
fi

pm_platform_helper "$GAMEDIR/bin/toluca-runtime"

EXTRA_ARGS=()
if [ -n "${TOLUCA_EXTRA_ARGS:-}" ]; then
  read -r -a EXTRA_ARGS <<< "$TOLUCA_EXTRA_ARGS"
fi

"$GAMEDIR/bin/toluca-runtime" \
  --fullscreen \
  --width "$WIDTH" \
  --height "$HEIGHT" \
  --aspect "$ASPECT" \
  --game-dir "$GAME_DATA_DIR" \
  --shader-cache "$GAMEDIR/cache/toluca-progs.bin" \
  "${EXTRA_ARGS[@]}" \
  "$EXE_PATH" \
  "$CACHE_BIN"

pm_finish
