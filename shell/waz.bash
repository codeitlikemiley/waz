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

# ── Natural language via command_not_found_handle ───────────────────
command_not_found_handle() {
    local full_input="$*"

    if command waz check-nl -- $full_input 2>/dev/null; then
        local response
        response=$(command waz ask --cwd "$PWD" --session "$WAZ_SESSION_ID" -- $full_input 2>/dev/null)

        if [[ -n "$response" ]]; then
            local suggested_cmd=""
            local display_text=""

            if [[ "$response" == *"__WAZ_CMD__:"* ]]; then
                display_text="${response%%__WAZ_CMD__:*}"
                suggested_cmd="${response##*__WAZ_CMD__:}"
                suggested_cmd="${suggested_cmd%%$'\n'*}"
                suggested_cmd="${suggested_cmd# }"
            else
                display_text="$response"
            fi

            echo ""
            echo -e "\033[0;33m🔮 waz:\033[0m"
            echo "$display_text" | sed 's/^/  /'

            if [[ -n "$suggested_cmd" ]]; then
                echo ""
                echo -e "\033[0;32m  → $suggested_cmd\033[0m"
                echo ""
                read -rp $'\033[0;90m  Run this command? [Y/n] \033[0m' reply
                if [[ "$reply" =~ ^[Yy]?$ ]]; then
                    eval "$suggested_cmd"
                fi
            fi

            return 0
        fi
    fi

    echo "bash: command not found: $1"
    return 127
}
