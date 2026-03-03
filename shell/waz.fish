# Waz — Command prediction for Fish
# Add to your ~/.config/fish/config.fish: waz init fish | source

# ── Session setup ───────────────────────────────────────────────────
if not set -q WAZ_SESSION_ID
    set -gx WAZ_SESSION_ID (waz session-id)
end

# ── Record commands after execution ─────────────────────────────────
function _waz_postexec --on-event fish_postexec
    set -l cmd $argv[1]
    set -l exit_code $status
    if test -n "$cmd"
        command waz record --cwd "$PWD" --session "$WAZ_SESSION_ID" --exit-code "$exit_code" -- "$cmd" &>/dev/null &
    end
end

# ── Custom keybinding for predictions ───────────────────────────────
function _waz_predict
    set -l buf (commandline --current-buffer)
    if test -n "$buf"
        set -l prediction (command waz predict --cwd "$PWD" --session "$WAZ_SESSION_ID" --prefix "$buf" 2>/dev/null)
        if test -n "$prediction"
            commandline --replace "$prediction"
            commandline --cursor (string length "$prediction")
        end
    end
end

# Bind Ctrl+Space to prediction
bind \c@ '_waz_predict'

# ── Natural language via fish_command_not_found ─────────────────────
function fish_command_not_found
    set -l full_input $argv

    if command waz check-nl -- $full_input 2>/dev/null
        set -l response (command waz ask --cwd "$PWD" --session "$WAZ_SESSION_ID" -- $full_input 2>/dev/null)

        if test -n "$response"
            echo ""
            set_color yellow
            echo "🔮 waz:"
            set_color normal
            echo "$response" | sed 's/^/  /' | sed 's/__WAZ_CMD__:.*//'
            
            # Extract suggested command
            set -l cmd_line (echo "$response" | grep '__WAZ_CMD__:' | sed 's/.*__WAZ_CMD__://')
            if test -n "$cmd_line"
                echo ""
                set_color green
                echo "  → $cmd_line"
                set_color normal
                echo ""
                read -P (set_color brblack)"  Run this command? [Y/n] "(set_color normal) reply
                if test -z "$reply" -o "$reply" = "Y" -o "$reply" = "y"
                    eval $cmd_line
                end
            end

            return 0
        end
    end

    echo "fish: Unknown command: $argv[1]"
    return 127
end
