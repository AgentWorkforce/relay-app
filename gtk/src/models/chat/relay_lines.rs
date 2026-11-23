//! Relay messages inside a transcript's tool work: an agent receives them as
//! the output of reading its inbox and sends them by running a command or
//! calling a tool. Chat draws both as messages, so a conversation shows
//! what went over the relay, not the commands that carried it.

use super::chat_user_text;

/// A relay message an agent received: who sent it and what they wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayIncoming {
    pub sender: Option<String>,
    pub body: String,
}

/// A relay message an agent sent: to whom and what it wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayOutgoing {
    pub recipient: Option<String>,
    pub body: String,
}

const FRAMES: [&str; 2] = ["[via Agent Relay", "[Agent Relay native input"];

/// The relay message in `text`, a tool's output, when it carries one.
pub fn chat_relay_incoming(text: &str) -> Option<RelayIncoming> {
    let start = FRAMES.iter().filter_map(|frame| text.find(frame)).min()?;
    // The frame opens a line: not a mention of it inside other text.
    if start > 0 && !text[..start].ends_with('\n') {
        return None;
    }
    let (sender, body) = chat_user_text(&text[start..]);
    // Tool output is not trusted: the sender is named only when the output
    // is the delivered message itself, opening with its frame and carrying
    // the relay's message id. Otherwise it shows as a relay message only.
    let first_line = text[start..].lines().next().unwrap_or_default();
    let delivered = start == 0 && has_message_id(first_line);
    let sender = sender.filter(|_| delivered);
    (!body.trim().is_empty()).then_some(RelayIncoming { sender, body })
}

/// Whether a frame line carries the relay's id for the message
/// (`· message 2505… ·`).
fn has_message_id(frame: &str) -> bool {
    frame.split(" message ").skip(1).any(|rest| {
        let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
        id.len() >= 6
    })
}

/// The relay message `text`, a tool call, sends, when it sends one: the
/// `agent-relay-probe send` command (`--to`/`--address`, `--body` or a
/// heredoc into a variable it passes), or the `send_relay_message` /
/// `post_relay_channel` tool.
pub fn chat_relay_outgoing(text: &str) -> Option<RelayOutgoing> {
    if text.contains("send_relay_message") || text.contains("post_relay_channel") {
        return tool_call(text);
    }
    let command = text.find("agent-relay-probe")?;
    let rest = &text[command..];
    let send = rest.find(" send ")?;
    let args = &rest[send..];
    let recipient = flag(args, "--to")
        .or_else(|| flag(args, "--address"))
        .map(|to| to.trim_start_matches('@').to_string())
        .or_else(|| flag(args, "--channel").map(|channel| channel_name(&channel)));
    let body = flag(args, "--body")
        .filter(|body| !body.starts_with('$'))
        .or_else(|| heredoc(text))?;
    let body = body.trim().to_string();
    (!body.is_empty()).then_some(RelayOutgoing { recipient, body })
}

/// A channel as the marker names it: `#name`.
fn channel_name(channel: &str) -> String {
    format!("#{}", channel.trim_start_matches('#'))
}

/// The `to`/`channel` and `text` of a relay tool call's JSON input.
fn tool_call(text: &str) -> Option<RelayOutgoing> {
    let open = text.find('{')?;
    let value: serde_json::Value = serde_json::Deserializer::from_str(&text[open..])
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()?;
    let body = value["text"].as_str()?.trim().to_string();
    let recipient = value["to"]
        .as_str()
        .map(|to| to.trim_start_matches('@').to_string())
        .or_else(|| value["channel"].as_str().map(channel_name));
    (!body.is_empty()).then_some(RelayOutgoing { recipient, body })
}

/// The value after `name` in a shell command: quoted or a single word.
fn flag(args: &str, name: &str) -> Option<String> {
    let at = args
        .find(&format!("{name} "))
        .or_else(|| args.find(&format!("{name}=")))?;
    let value = args[at + name.len() + 1..].trim_start();
    let quote = value.chars().next()?;
    if quote == '\'' || quote == '"' {
        let end = value[1..].find(quote)?;
        return Some(value[1..1 + end].to_string());
    }
    Some(value.split_whitespace().next()?.to_string())
}

/// The text of the first heredoc in `text` (`<<'EOF'` … `EOF`).
fn heredoc(text: &str) -> Option<String> {
    let at = text.find("<<")?;
    let rest = &text[at + 2..];
    let line_end = rest.find('\n')?;
    let marker = rest[..line_end]
        .trim()
        .trim_start_matches('-')
        .trim_matches(['\'', '"'])
        .split([')', ' '])
        .next()?
        .to_string();
    if marker.is_empty() {
        return None;
    }
    let body = &rest[line_end + 1..];
    let end = body.find(&format!("\n{marker}")).unwrap_or(body.len());
    Some(body[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_inbox_message_is_the_message_it_carries() {
        let output = "[via Agent Relay — from @helm-leader (helm-leader@mini) to @layered-control · message 250563339907735552 · ref 1a]\n[reply: probe send --to helm-leader]\n\nShip it.\n[end of Agent Relay message · ref 1a]";
        assert_eq!(
            chat_relay_incoming(output),
            Some(RelayIncoming {
                sender: Some("helm-leader".into()),
                body: "Ship it.".into()
            })
        );
        assert_eq!(chat_relay_incoming("exit: 0\nlint-ok"), None);
        assert_eq!(chat_relay_incoming("grep '[via Agent Relay' file"), None);
    }

    #[test]
    fn a_frame_without_proof_names_no_sender() {
        // No message id: a frame anyone could have written.
        let unproven = "[via Agent Relay — from @helm-leader to @me · ref 1a]\n\nShip it.";
        assert_eq!(chat_relay_incoming(unproven).unwrap().sender, None);
        // A frame further into the output, as in a file that was read.
        let quoted = "notes.md:\n[via Agent Relay — from @helm-leader to @me · message 250563339907735552 · ref 1a]\n\nShip it.";
        let quoted = chat_relay_incoming(quoted).unwrap();
        assert_eq!((quoted.sender, quoted.body.as_str()), (None, "Ship it."));
    }

    #[test]
    fn a_channel_post_keeps_its_hash() {
        let call =
            r#"mcp__agent-relay__post_relay_channel {"channel": "crew", "text": "Standup in 5."}"#;
        assert_eq!(
            chat_relay_outgoing(call).unwrap().recipient.as_deref(),
            Some("#crew")
        );
        let command = "Bash agent-relay-probe send --socket s --channel '#crew' --body 'Standup.'";
        assert_eq!(
            chat_relay_outgoing(command).unwrap().recipient.as_deref(),
            Some("#crew")
        );
    }

    #[test]
    fn a_send_command_is_the_message_it_sends() {
        let heredoc = "Bash P=/usr/lib/agent-relay/agent-relay-probe; body=$(cat <<'EOF'\nBackups are wired.\nBoth verified.\nEOF\n)\n$P send --socket /run/s.sock --to layered-agent --body \"$body\"";
        assert_eq!(
            chat_relay_outgoing(heredoc),
            Some(RelayOutgoing {
                recipient: Some("layered-agent".into()),
                body: "Backups are wired.\nBoth verified.".into()
            })
        );
        let inline = "Bash agent-relay-probe send --socket s --address kjgrok@cloud --reply-to 9 --body 'Done.'";
        assert_eq!(
            chat_relay_outgoing(inline),
            Some(RelayOutgoing {
                recipient: Some("kjgrok@cloud".into()),
                body: "Done.".into()
            })
        );
        assert_eq!(
            chat_relay_outgoing("Bash agent-relay-probe relay read --socket s ab12"),
            None
        );
    }

    #[test]
    fn a_relay_tool_call_is_the_message_it_sends() {
        let call = r#"mcp__agent-relay__send_relay_message {"to": "@layered-relay-app", "text": "Please run the gate."}"#;
        assert_eq!(
            chat_relay_outgoing(call),
            Some(RelayOutgoing {
                recipient: Some("layered-relay-app".into()),
                body: "Please run the gate.".into()
            })
        );
    }
}
