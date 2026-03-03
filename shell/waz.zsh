# Waz — Command prediction for Zsh
# Usage: eval "$(waz init zsh)"

# --- Session setup ---
export WAZ_SESSION_ID="${WAZ_SESSION_ID:-$(command waz session-id)}"

# --- Record commands after execution ---
_waz_preexec() {
    _WAZ_LAST_CMD="$1"
}

_waz_precmd() {
    local exit_code=$?
    if [[ -n "$_WAZ_LAST_CMD" ]]; then
        command waz record --cwd "$PWD" --session "$WAZ_SESSION_ID" --exit-code "$exit_code" -- "$_WAZ_LAST_CMD" &>/dev/null &!
        _WAZ_LAST_CMD=""
    fi
    # Flag: show proactive suggestion when the new prompt appears
    _WAZ_SHOW_PROACTIVE=1
}

autoload -Uz add-zsh-hook
add-zsh-hook preexec _waz_preexec
add-zsh-hook precmd _waz_precmd

# --- Ghost text prediction via ZLE ---
typeset -g _WAZ_SUGGESTION=""
typeset -g _WAZ_SHOW_PROACTIVE=""

_waz_suggest() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""
    region_highlight=("${(@)region_highlight:#*fg=#6c6c6c*}")

    local buf="$BUFFER"
    local prediction

    if [[ -z "${buf// /}" ]]; then
        # Empty buffer: proactive prediction (no prefix)
        prediction=$(command waz predict --cwd "$PWD" --session "$WAZ_SESSION_ID" 2>/dev/null)
    else
        # User is typing: prefix-based prediction
        prediction=$(command waz predict --cwd "$PWD" --session "$WAZ_SESSION_ID" --prefix "$buf" 2>/dev/null)
    fi

    if [[ -n "$prediction" && "$prediction" != "$buf" ]]; then
        _WAZ_SUGGESTION="$prediction"
        if [[ -z "${buf// /}" ]]; then
            # Empty buffer: show full command as ghost text
            POSTDISPLAY="$prediction"
        elif [[ "$prediction" == "$buf"* ]]; then
            # Prefix match: show only the completion part
            POSTDISPLAY="${prediction#$buf}"
        else
            # Non-prefix match: show with arrow
            POSTDISPLAY="  ← $prediction"
        fi
        # Style the ghost text as dim gray
        local start=${#BUFFER}
        local end=$((start + ${#POSTDISPLAY}))
        region_highlight+=("$start $end fg=#6c6c6c")
    fi
}

_waz_clear() {
    _WAZ_SUGGESTION=""
    POSTDISPLAY=""
}

# --- Widget: show proactive suggestion when line editor starts ---
_waz_line_init() {
    if [[ -n "$_WAZ_SHOW_PROACTIVE" ]]; then
        _WAZ_SHOW_PROACTIVE=""
        _waz_suggest
    fi
}

# --- Widget: self-insert + suggest ---
_waz_self_insert() {
    zle .self-insert
    _waz_suggest
}

# --- Widget: backward-delete + suggest ---
_waz_backward_delete() {
    zle .backward-delete-char
    _waz_suggest
}

# --- Widget: accept full suggestion ---
_waz_accept() {
    if [[ -n "$_WAZ_SUGGESTION" ]]; then
        BUFFER="$_WAZ_SUGGESTION"
        CURSOR=${#BUFFER}
        _waz_clear
    else
        zle .forward-char
    fi
}

# --- Widget: accept one word ---
_waz_accept_word() {
    if [[ -n "$_WAZ_SUGGESTION" && "$_WAZ_SUGGESTION" != "$BUFFER" ]]; then
        local remaining="${_WAZ_SUGGESTION#$BUFFER}"
        local next_word="${remaining%% *}"
        if [[ "$remaining" == "$next_word" ]]; then
            BUFFER="$_WAZ_SUGGESTION"
        else
            BUFFER="${BUFFER}${next_word} "
        fi
        CURSOR=${#BUFFER}
        _waz_suggest
    else
        zle .forward-word
    fi
}

# --- Widget: accept-line clears ghost text ---
_waz_accept_line() {
    _waz_clear
    zle .accept-line
}

# --- Widget: ctrl-c clears ghost text ---
_waz_send_break() {
    _waz_clear
    zle .send-break
}

# --- Register all widgets ---
zle -N self-insert _waz_self_insert
zle -N backward-delete-char _waz_backward_delete
zle -N _waz_accept
zle -N _waz_accept_word
zle -N accept-line _waz_accept_line
zle -N send-break _waz_send_break
zle -N zle-line-init _waz_line_init

# --- Keybindings ---
bindkey '^[[C' _waz_accept
bindkey '^[f'  _waz_accept_word
