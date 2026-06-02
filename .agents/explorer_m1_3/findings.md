# Findings: `TmpFormPanel` and Completion Integration Investigation

## 1. File Definitions and Locations
- **`InputSuggestionsMode::TmpFormPanel` Definition**: Located in `/Volumes/goldcoders/zap/app/src/terminal/input.rs` as a variant of the `InputSuggestionsMode` enum.
- **Form UI Rendering**: The form layout, labels, and command prefix are rendered via `render_tmp_form_panel` inside `/Volumes/goldcoders/zap/app/src/terminal/tmp_panel.rs`.
- **Suggestions Menu Overlay**: Renders standard suggestion items inside a popup container via `render_completion_suggestions_menu` in `/Volumes/goldcoders/zap/app/src/terminal/input/suggestions_mode_menu.rs`.
- **Command Assembly & Token Parsing**: The schema for commands/arguments, token values extraction, and command string assembly is implemented in `/Volumes/goldcoders/zap/crates/warp_completer/src/signatures/tmp.rs`.

---

## 2. Event Handling and Navigation Logic

### Tab Navigation (`input_tab` in `/Volumes/goldcoders/zap/app/src/terminal/input.rs`)
When a Tab keypress occurs under `TmpFormPanel` mode:
1. **TokenType::Boolean**: Cycles the token value between `"true"` and `"false"`, updates the editor buffer, and saves the new state.
2. **TokenType::Enum**: Cycles through the allowed variant values specified in the token definition, updates the editor buffer, and saves the new state.
3. **TokenType::File**:
   - If the suggestion list `input_suggestions` is empty, it opens the suggestion list via `open_completion_suggestions`.
   - If the suggestion list is not empty, it cycles to the **next** suggestion via `suggestions.select_next(ctx)`, updates the editor buffer, and updates the token state.
4. **Other Types**: Advances the focus to the next token index (`active_token_index += 1`).

### Shift-Tab Navigation (`input_shift_tab` in `/Volumes/goldcoders/zap/app/src/terminal/input.rs`)
When Shift-Tab occurs under `TmpFormPanel` mode:
1. It **always** moves focus to the previous token index (`active_token_index -= 1`) and saves the state.
2. **Crucially, it completely ignores the completions menu overlay**, even if the current token is `FileType` and `input_suggestions` is active and has items.

### Arrow Key Navigation (`editor_up`/`editor_down` in `/Volumes/goldcoders/zap/app/src/terminal/input.rs`)
When Arrow Down (or Arrow Up) occurs:
1. If `input_suggestions` is **not empty**:
   - It cycles the selection in `input_suggestions` (`s.select_next` or `s.select_prev`).
   - It retrieves the highlighted item, replaces the current active token's value in the state, builds the assembled command, updates the editor buffer text programmatically (`editor.set_buffer_text_ignoring_undo`), and updates the mode model.
2. If `input_suggestions` is **empty**:
   - It acts as field navigation, incrementing or decrementing `active_token_index` to move between form arguments.

### Suggestion Confirmation (`InputSuggestionsEvent::ConfirmSuggestion` & `input_enter`)
1. **Mouse Clicks**: Clicking on an item in the suggestion overlay dispatches `SelectAndConfirm(index)`, which emits `InputSuggestionsEvent::Select` followed immediately by `ConfirmSuggestion`.
2. **Enter Key**: Handled by `input_enter` in `/Volumes/goldcoders/zap/app/src/terminal/input.rs`.
   - When `TmpFormPanel` is active, it immediately performs validation of required arguments and executes the command.
   - **Crucially, it does not check if the completions menu overlay is open**, meaning that pressing Enter bypasses suggestion selection and immediately executes the whole command.

---

## 3. Why Selections & Buffer Updates go Out of Sync (Root Cause Analysis)

There are three main flaws causing the state and buffer to fall out of sync:

### Cause A: System-initiated Edits Loopback and Lossy Parsing
1. When a completion is cycled (via arrow keys or tab/click selection), the handler performs a programmatic update:
   `editor.set_buffer_text_ignoring_undo(&assembled, ctx);`
2. This programmatic update emits `EditorEvent::Edited(edit_origin)` with a non-user origin (`EditOrigin::SystemEdit`).
3. `handle_editor_event` intercepts the edit, matching the `InputSuggestionsMode::TmpFormPanel` arm. It then parses the new buffer text back to extract values:
   `let new_vals = extract_token_values(&entry.command, &entry.tokens, &buffer_text);`
4. Because the assembled buffer text has placeholders stripped (e.g. `<arg>` has been removed or contains empty spaces), the parsing logic in `extract_token_values` often mis-aligns or returns incorrect values.
5. If `new_vals != *token_values`, it calls `set_mode` to save these corrupted values back into the mode state, overwriting the clean, updated state that was just selected from the completions menu.

### Cause B: Enter Key Bypassing Suggestion Confirmation
In `input_enter`, the `TmpFormPanel` branch executes first and has no awareness of the completion menu overlay's open state:
```rust
        if let InputSuggestionsMode::TmpFormPanel {
            command_entry,
            active_token_index: _,
            token_values,
        } = self.suggestions_mode_model.as_ref(ctx).mode().clone() {
            // ... validates and executes command immediately ...
        }
```
If a completions overlay is open (e.g. for choosing files), pressing Enter should confirm the selected suggestion instead of executing the command.

### Cause C: Shift-Tab Bypassing Suggestion Cycling
In `input_shift_tab`, the `TmpFormPanel` arm instantly changes the active token index without checking if the suggestions list is open, preventing the user from cycling backwards through completion items.

---

## 4. Remediation Plan

To solve these issues, the following fixes are proposed:

### Fix A: Only Re-parse Buffer on User-initiated Edits
In `/Volumes/goldcoders/zap/app/src/terminal/input.rs`, inside `handle_editor_event`, under `InputSuggestionsMode::TmpFormPanel`, restrict token parsing to user-initiated typing:
```rust
                    InputSuggestionsMode::TmpFormPanel {
                        command_entry,
                        active_token_index,
                        token_values,
                    } => {
                        // Only parse and update token values if the edit was initiated by the user typing,
                        // avoiding corrupting the state when programmatic system edits (selections) update the buffer.
                        if edit_origin.is_user() {
                            let buffer_text = self.buffer_text(ctx);
                            let entry = command_entry.clone();
                            let active_idx = *active_token_index;
                            
                            let cwd = self.active_block_metadata
                                .as_ref()
                                .and_then(|m| m.current_working_directory())
                                .unwrap_or("");
                            let matched = warp_completer::signatures::tmp::find_matching_tmp_command(&buffer_text, cwd);
                            if let Some((matched_entry, _prefix)) = matched {
                                if matched_entry.command != entry.command {
                                    let new_vals = warp_completer::signatures::tmp::extract_token_values(&matched_entry.command, &matched_entry.tokens, &buffer_text);
                                    self.suggestions_mode_model.update(ctx, |m, ctx| {
                                        m.set_mode(
                                            InputSuggestionsMode::TmpFormPanel {
                                                command_entry: matched_entry,
                                                active_token_index: 0,
                                                token_values: new_vals,
                                            },
                                            ctx,
                                        );
                                    });
                                } else {
                                    let new_vals = warp_completer::signatures::tmp::extract_token_values(&entry.command, &entry.tokens, &buffer_text);
                                    if new_vals != *token_values {
                                        self.suggestions_mode_model.update(ctx, |m, ctx| {
                                            m.set_mode(
                                                InputSuggestionsMode::TmpFormPanel {
                                                    command_entry: entry.clone(),
                                                    active_token_index: active_idx,
                                                    token_values: new_vals,
                                                },
                                                ctx,
                                            );
                                        });
                                    }
                                    if let Some(token) = entry.tokens.get(active_idx) {
                                        if token.token_type == warp_completer::signatures::tmp::TokenType::File {
                                            self.open_completion_suggestions(CompletionsTrigger::AsYouType, ctx);
                                        }
                                    }
                                }
                            } else {
                                self.close_input_suggestions(/*should_focus_input=*/ true, ctx);
                            }
                        }
                    }
```

### Fix B: Support Enter Confirmation for Open Overlay in `input_enter`
In `/Volumes/goldcoders/zap/app/src/terminal/input.rs`, inside `input_enter`, verify if the suggestion list is non-empty before executing:
```rust
        if let InputSuggestionsMode::TmpFormPanel {
            command_entry,
            active_token_index,
            token_values,
        } = self.suggestions_mode_model.as_ref(ctx).mode().clone() {
            // If the completions overlay is currently open with items, Enter should select and confirm
            // the suggestion rather than executing the entire command immediately.
            if !self.input_suggestions.as_ref(ctx).is_empty() && self.should_enter_accept_completion_suggestion(ctx) {
                self.input_suggestions.update(ctx, |suggestions, ctx| {
                    suggestions.confirm(ctx);
                });
                return;
            }

            for (i, token) in command_entry.tokens.iter().enumerate() {
                // ... validation & execution logic ...
            }
        }
```

### Fix C: Support Shift-Tab Cycling in `input_shift_tab`
In `/Volumes/goldcoders/zap/app/src/terminal/input.rs`, inside `input_shift_tab`, check if the overlay is active:
```rust
            InputSuggestionsMode::TmpFormPanel {
                command_entry,
                active_token_index,
                token_values,
            } => {
                let entry = command_entry.clone();
                let mut vals = token_values.clone();
                let idx = *active_token_index;

                // If suggestions menu is open, cycle backwards through suggestions instead of switching fields
                if idx < entry.tokens.len() 
                    && entry.tokens[idx].token_type == warp_completer::signatures::tmp::TokenType::File 
                    && !self.input_suggestions.as_ref(ctx).is_empty() 
                {
                    self.input_suggestions.update(ctx, |suggestions, ctx| {
                        suggestions.select_prev(ctx);
                    });
                    if let Some(selected_text) = self.input_suggestions.as_ref(ctx).get_selected_item_text() {
                        vals[idx] = selected_text.to_string();
                        let assembled = warp_completer::signatures::tmp::build_assembled_command(&entry, &vals, false);
                        self.editor.update(ctx, |editor, ctx| {
                            editor.set_buffer_text_ignoring_undo(&assembled, ctx);
                            let char_len = editor.buffer_text(ctx).chars().count() as u32;
                            editor.reset_selections_to_point(&BufferPoint::new(0, char_len), ctx);
                        });
                        self.suggestions_mode_model.update(ctx, |m, ctx| {
                            m.set_mode(
                                InputSuggestionsMode::TmpFormPanel {
                                    command_entry: entry,
                                    active_token_index: idx,
                                    token_values: vals,
                                },
                                ctx,
                            );
                        });
                    }
                    return;
                }

                // Default field navigation behavior
                let mut idx = idx;
                if idx > 0 {
                    idx -= 1;
                }
                self.suggestions_mode_model.update(ctx, |m, ctx| {
                    m.set_mode(
                        InputSuggestionsMode::TmpFormPanel {
                            command_entry: entry,
                            active_token_index: idx,
                            token_values: vals,
                        },
                        ctx,
                    );
                });
                return;
            }
```
