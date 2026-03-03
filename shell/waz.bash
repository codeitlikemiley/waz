#!/usr/bin/env bash
# Waz — Command prediction for Bash
# Add to your ~/.bashrc: eval "$(waz init bash)"

# ── Session setup ───────────────────────────────────────────────────
export WAZ_SESSION_ID="${WAZ_SESSION_ID:-$(waz session-id)}"

# ── Record commands after execution ─────────────────────────────────
_waz_last_histcmd=""

_waz_prompt_command() {
    local exit_code=$?

    # Get the latest history entry
    local current_histcmd
    current_histcmd=$(HISTTIMEFORMAT= history 1 | sed 's/^[ ]*[0-9]*[ ]*//')

    # Only record if it's a new command (history number changed)
    if [[ -n "$current_histcmd" && "$current_histcmd" != "$_waz_last_histcmd" ]]; then
        command waz record --cwd "$PWD" --session "$WAZ_SESSION_ID" --exit-code "$exit_code" -- "$current_histcmd" &>/dev/null &
        _waz_last_histcmd="$current_histcmd"
    fi
}

# Append to existing PROMPT_COMMAND
if [[ -z "$PROMPT_COMMAND" ]]; then
    PROMPT_COMMAND="_waz_prompt_command"
else
    PROMPT_COMMAND="_waz_prompt_command;${PROMPT_COMMAND}"
fi

# ── Inline suggestion via readline ──────────────────────────────────
_waz_predict_fill() {
    local prediction
    prediction=$(command waz predict --cwd "$PWD" --session "$WAZ_SESSION_ID" --prefix "$READLINE_LINE" 2>/dev/null)

    if [[ -n "$prediction" ]]; then
        READLINE_LINE="$prediction"
        READLINE_POINT=${#READLINE_LINE}
    fi
}

# Bind Ctrl+Space to trigger prediction fill
bind -x '"\C-@": _waz_predict_fill'
