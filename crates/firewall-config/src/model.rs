//! What the tool knows about the firewall: the rules read from
//! `ufw show added`, the form of a new rule and the queue of changes. Nothing
//! here runs a command.

use std::net::IpAddr;

/// Markers between the parts of the output of the privileged script.
pub const MARK_STATUS: &str = "@@opusdm-status";
pub const MARK_DEFAULTS: &str = "@@opusdm-defaults";
pub const MARK_RULES: &str = "@@opusdm-rules";
pub const MARK_OP_OK: &str = "@@opusdm-op-ok";
pub const MARK_OP_FAILED: &str = "@@opusdm-op-failed";

/// The highest port number.
const MAX_PORT: u32 = 65535;

/// Which way a rule looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

/// The protocol of a new rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
    Both,
}

/// The action of a new rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Allow,
    Deny,
}

/// One rule as `ufw show added` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The words after `ufw`, exactly as listed: they delete the rule.
    pub words: Vec<String>,
    pub direction: Direction,
    pub action: String,
    pub port: String,
    pub proto: String,
    /// The other side: From on an IN rule, To on an OUT rule.
    pub peer: String,
    /// Everything else the rule says, so nothing is hidden.
    pub notes: String,
}

/// What the firewall looks like at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub active: bool,
    pub default_in: String,
    pub default_out: String,
    pub rules: Vec<Rule>,
}

/// One `ufw` command of a privileged run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub args: Vec<String>,
}

/// Split a line of `ufw show added` into words. Only single quotes group, as
/// ufw prints them around a comment; an open quote takes the rest of the line.
pub fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut has_word) = (false, false);
    for c in line.chars() {
        match c {
            '\'' => {
                quoted = !quoted;
                has_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_word { words.push(std::mem::take(&mut word)); }
                has_word = false;
            }
            c => {
                word.push(c);
                has_word = true;
            }
        }
    }
    if has_word { words.push(word); }
    words
}

/// Read a rule from its words (without the leading `ufw`). Words that are not
/// understood go to the notes: the rule is shown and can be removed anyway.
pub fn parse_rule(words: Vec<String>) -> Rule {
    let mut rule = Rule { words: words.clone(), direction: Direction::In, action: String::new(), port: "any".into(), proto: "any".into(), peer: "anywhere".into(), notes: String::new() };
    let mut notes: Vec<String> = Vec::new();
    let mut rest = words.into_iter().peekable();
    let is_route = rest.peek().is_some_and(|w| w == "route");
    if is_route {
        rest.next();
        notes.push("route".into());
    }
    rule.action = rest.next().unwrap_or_default();
    while let Some(word) = rest.peek().cloned() {
        match word.as_str() {
            "log" | "log-all" => {
                notes.push(word);
                rest.next();
                continue;
            }
            "in" => rule.direction = Direction::In,
            "out" => rule.direction = Direction::Out,
            _ => break,
        }
        rest.next();
        if rest.peek().is_some_and(|w| w == "on") {
            rest.next();
            let iface = rest.next().unwrap_or_default();
            notes.push(format!("{word} on {iface}"));
        }
    }
    let (mut from, mut to, mut from_port) = (None, None, None);
    let mut simple_seen = false;
    let mut last_side = "";
    while let Some(word) = rest.next() {
        match word.as_str() {
            "from" => { from = rest.next(); last_side = "from"; }
            "to" => { to = rest.next(); last_side = "to"; }
            "port" if last_side == "from" => from_port = rest.next(),
            "port" => rule.port = rest.next().unwrap_or_default(),
            "proto" => rule.proto = rest.next().unwrap_or_default(),
            "comment" => notes.push(format!("comment: {}", rest.next().unwrap_or_default())),
            _ if !simple_seen && last_side.is_empty() => {
                simple_seen = true;
                match word.split_once('/') {
                    Some((port, proto)) => {
                        rule.port = port.to_string();
                        rule.proto = proto.to_string();
                    }
                    None => rule.port = word,
                }
            }
            _ => notes.push(word),
        }
    }
    // A route rule names both directions: it is listed under IN.
    if is_route { rule.direction = Direction::In; }
    let is_any = |side: &Option<String>| side.as_deref().is_none_or(|s| s == "any");
    let (peer, other, other_name) = match rule.direction {
        Direction::In => (&from, &to, "to"),
        Direction::Out => (&to, &from, "from"),
    };
    if !is_any(peer) { rule.peer = peer.clone().unwrap_or_default(); }
    if !is_any(other) { notes.push(format!("{other_name} {}", other.clone().unwrap_or_default())); }
    if let Some(port) = from_port { notes.push(format!("source port {port}")); }
    rule.notes = notes.join(", ");
    rule
}

/// Read the name of a default policy from the text of `/etc/default/ufw`.
fn parse_policy(defaults: &str, key: &str) -> String {
    let value = defaults.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=').map(|v| v.trim().trim_matches('"').to_string()));
    match value.as_deref() {
        Some("DROP") => "deny".into(),
        Some("ACCEPT") => "allow".into(),
        Some("REJECT") => "reject".into(),
        Some(other) => other.to_lowercase(),
        None => "unknown".into(),
    }
}

/// Read the state from the output of the privileged script (from
/// [`MARK_STATUS`] on). `Err` holds the text to show when `ufw` did not answer
/// as expected: a failed read must never look like "no rules".
pub fn parse_snapshot(output: &str) -> Result<Snapshot, String> {
    let (_, tail) = output.split_once(MARK_STATUS).ok_or("the privileged script printed no status")?;
    let (status, tail) = tail.split_once(MARK_DEFAULTS).ok_or("the privileged script printed no default policies")?;
    let (defaults, rules) = tail.split_once(MARK_RULES).ok_or("the privileged script printed no rules")?;
    let status_line = status.lines().find_map(|l| l.trim().strip_prefix("Status:")).map(str::trim);
    let active = match status_line {
        Some("active") => true,
        Some("inactive") => false,
        _ => return Err(format!("ufw status: {}", status.trim())),
    };
    let lines: Vec<&str> = rules.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if !lines.iter().any(|l| l.starts_with("Added user rules")) { return Err(format!("ufw show added: {}", rules.trim())); }
    let rules = lines.iter().filter_map(|l| l.strip_prefix("ufw ")).map(|l| parse_rule(split_words(l))).collect();
    Ok(Snapshot { active, default_in: parse_policy(defaults, "DEFAULT_INPUT_POLICY"), default_out: parse_policy(defaults, "DEFAULT_OUTPUT_POLICY"), rules })
}

/// Whether `text` is an IP address or a network in CIDR form.
fn valid_address(text: &str) -> bool {
    let Some((ip, bits)) = text.split_once('/') else { return text.parse::<IpAddr>().is_ok() };
    let Ok(ip) = ip.parse::<IpAddr>() else { return false };
    let Ok(bits) = bits.parse::<u8>() else { return false };
    bits <= if ip.is_ipv4() { 32 } else { 128 }
}

/// A checked new rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRule {
    pub direction: Direction,
    pub port: u16,
    pub proto: Protocol,
    pub action: Action,
    /// The other side; `None` means anywhere.
    pub address: Option<String>,
}

impl NewRule {
    /// Check what the form holds. `Err` is the text to show under it.
    pub fn from_form(direction: Direction, port: &str, proto: Protocol, action: Action, address: &str) -> Result<NewRule, String> {
        let port_text = port.trim();
        let port = port_text.parse::<u32>().ok().filter(|p| (1..=MAX_PORT).contains(p)).ok_or_else(|| format!("The port must be a number from 1 to {MAX_PORT}."))?;
        let address = address.trim();
        if !address.is_empty() && !valid_address(address) { return Err("The address must be an IP address or a network like 192.168.1.0/24.".into()); }
        Ok(NewRule { direction, port: port as u16, proto, action, address: (!address.is_empty()).then(|| address.to_string()) })
    }

    /// The `ufw` arguments that add this rule.
    pub fn args(&self) -> Vec<String> {
        let action = match self.action { Action::Allow => "allow", Action::Deny => "deny" };
        let dir = match self.direction { Direction::In => "in", Direction::Out => "out" };
        let proto = match self.proto { Protocol::Tcp => Some("tcp"), Protocol::Udp => Some("udp"), Protocol::Both => None };
        let mut args = vec![action.to_string(), dir.to_string()];
        let Some(address) = &self.address else {
            args.push(match proto { Some(p) => format!("{}/{p}", self.port), None => self.port.to_string() });
            return args;
        };
        match self.direction {
            Direction::In => args.extend(["from".to_string(), address.clone(), "to".to_string(), "any".to_string()]),
            Direction::Out => args.extend(["to".to_string(), address.clone()]),
        }
        args.extend(["port".to_string(), self.port.to_string()]);
        if let Some(p) = proto { args.extend(["proto".to_string(), p.to_string()]); }
        args
    }

    /// The rule as it will look in the table.
    pub fn preview(&self) -> Rule {
        parse_rule(self.args())
    }
}

/// The changes waiting for Apply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Queue {
    /// The words of the rules to remove.
    pub removed: Vec<Vec<String>>,
    pub added: Vec<NewRule>,
    /// Turn the firewall on (`true`) or off (`false`).
    pub power: Option<bool>,
}

impl Queue {
    /// Whether there is nothing to apply.
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty() && self.power.is_none()
    }

    /// Queue `rule`; the same rule twice stays one.
    pub fn add(&mut self, rule: NewRule) {
        if !self.added.contains(&rule) { self.added.push(rule); }
    }

    /// Queue the removal of the rule with these words, or take it back.
    pub fn toggle_removed(&mut self, words: &[String]) {
        match self.removed.iter().position(|w| w == words) {
            Some(i) => { self.removed.remove(i); }
            None => self.removed.push(words.to_vec()),
        }
    }

    /// Ask to change the state of the firewall, now `active`; asking again
    /// takes it back.
    pub fn toggle_power(&mut self, active: bool) {
        self.power = match self.power {
            Some(_) => None,
            None => Some(!active),
        };
    }

    /// The commands of this queue, in order: removals, additions, on or off.
    pub fn ops(&self) -> Vec<Op> {
        let mut ops: Vec<Op> = self.removed.iter().map(|w| Op { args: delete_args(w) }).collect();
        ops.extend(self.added.iter().map(|r| Op { args: r.args() }));
        if let Some(on) = self.power {
            let word = if on { "enable" } else { "disable" };
            ops.push(Op { args: vec!["--force".to_string(), word.to_string()] });
        }
        ops
    }

    /// Take out the first `done` commands of [`ops`](Self::ops): they ran.
    pub fn drop_done(&mut self, done: usize) {
        let removed = done.min(self.removed.len());
        self.removed.drain(..removed);
        let added = (done - removed).min(self.added.len());
        self.added.drain(..added);
        if done > removed + added { self.power = None; }
    }
}

/// The `ufw` arguments that delete the rule with these words. A route rule has
/// its own form of delete.
fn delete_args(words: &[String]) -> Vec<String> {
    let mut args = vec!["--force".to_string()];
    match words.split_first() {
        Some((first, rest)) if first == "route" => {
            args.extend(["route".to_string(), "delete".to_string()]);
            args.extend_from_slice(rest);
        }
        _ => {
            args.push("delete".to_string());
            args.extend_from_slice(words);
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read one line as `ufw show added` prints it, without the `ufw`.
    fn rule(line: &str) -> Rule {
        parse_rule(split_words(line))
    }

    #[test]
    fn the_real_lines_of_show_added_are_read() {
        let r = rule("allow from 192.168.1.0/24 to any port 22 proto tcp");
        assert_eq!((r.direction, r.action.as_str(), r.port.as_str(), r.proto.as_str(), r.peer.as_str(), r.notes.as_str()), (Direction::In, "allow", "22", "tcp", "192.168.1.0/24", ""));
        let r = rule("deny out 25/tcp");
        assert_eq!((r.direction, r.action.as_str(), r.port.as_str(), r.proto.as_str(), r.peer.as_str()), (Direction::Out, "deny", "25", "tcp", "anywhere"));
        let r = rule("allow out to 1.1.1.1 port 53");
        assert_eq!((r.direction, r.port.as_str(), r.proto.as_str(), r.peer.as_str()), (Direction::Out, "53", "any", "1.1.1.1"));
        let r = rule("allow 1000:2000");
        assert_eq!((r.direction, r.port.as_str(), r.proto.as_str()), (Direction::In, "1000:2000", "any"));
        let r = rule("allow 6000:6007/tcp");
        assert_eq!((r.port.as_str(), r.proto.as_str()), ("6000:6007", "tcp"));
    }

    #[test]
    fn a_comment_is_one_word_and_is_kept_for_the_delete() {
        let r = rule("allow 8080/tcp comment 'my app'");
        assert_eq!(r.notes, "comment: my app");
        assert_eq!(r.words, ["allow", "8080/tcp", "comment", "my app"]);
    }

    #[test]
    fn what_the_form_cannot_make_is_still_shown_in_the_notes() {
        let r = rule("limit in on eth0 to any port 2222 proto tcp");
        assert_eq!((r.action.as_str(), r.port.as_str(), r.notes.as_str()), ("limit", "2222", "in on eth0"));
        let r = rule("route allow in on wg0 out on eth0");
        assert_eq!((r.direction, r.action.as_str()), (Direction::In, "allow"));
        assert_eq!(r.notes, "route, in on wg0, out on eth0");
        let r = rule("allow log OpenSSH");
        assert_eq!((r.port.as_str(), r.notes.as_str()), ("OpenSSH", "log"));
        let r = rule("allow from 10.0.0.1 port 1234 to 10.0.0.2 port 80");
        assert_eq!((r.port.as_str(), r.peer.as_str()), ("80", "10.0.0.1"));
        assert_eq!(r.notes, "to 10.0.0.2, source port 1234");
    }

    #[test]
    fn a_snapshot_needs_real_answers() {
        let good = format!("{MARK_STATUS}\nStatus: inactive\n{MARK_DEFAULTS}\nDEFAULT_INPUT_POLICY=\"DROP\"\nDEFAULT_OUTPUT_POLICY=\"ACCEPT\"\n{MARK_RULES}\nAdded user rules (see 'ufw status' for running firewall):\nufw deny out 25/tcp\n");
        let s = parse_snapshot(&good).unwrap();
        assert_eq!((s.active, s.default_in.as_str(), s.default_out.as_str(), s.rules.len()), (false, "deny", "allow", 1));
        let no_rules = format!("{MARK_STATUS}\nStatus: active\n{MARK_DEFAULTS}\n{MARK_RULES}\nAdded user rules (see 'ufw status' for running firewall):\n");
        assert!(parse_snapshot(&no_rules).unwrap().rules.is_empty());
        let bad_status = format!("{MARK_STATUS}\nERROR: problem running iptables\n{MARK_DEFAULTS}\n{MARK_RULES}\nAdded user rules:\n");
        assert!(parse_snapshot(&bad_status).unwrap_err().contains("iptables"));
        let bad_rules = format!("{MARK_STATUS}\nStatus: active\n{MARK_DEFAULTS}\n{MARK_RULES}\nERROR: cannot read\n");
        assert!(parse_snapshot(&bad_rules).unwrap_err().contains("cannot read"));
        assert!(parse_snapshot("").is_err());
    }

    #[test]
    fn the_form_checks_port_and_address() {
        let make = |port: &str, address: &str| NewRule::from_form(Direction::In, port, Protocol::Tcp, Action::Allow, address);
        for bad in ["", "0", "65536", "22;id", "-1", "ssh", "22 23"] { assert!(make(bad, "").is_err(), "port {bad:?}"); }
        for bad in ["nonsense", "1.2.3.4/33", "::1/129", "1.2.3.4;id", "--force", "1.2.3.4 5.6.7.8"] { assert!(make("22", bad).is_err(), "address {bad:?}"); }
        assert_eq!(make(" 22 ", "  ").unwrap().address, None);
        for good in ["1.2.3.4", "192.168.1.0/24", "::1", "fd00::/8"] { assert_eq!(make("65535", good).unwrap().address.as_deref(), Some(good)); }
    }

    #[test]
    fn a_new_rule_makes_the_commands_of_ufw() {
        let make = |dir, proto, address: &str| NewRule::from_form(dir, "22", proto, Action::Allow, address).unwrap().args().join(" ");
        assert_eq!(make(Direction::In, Protocol::Tcp, ""), "allow in 22/tcp");
        assert_eq!(make(Direction::In, Protocol::Both, ""), "allow in 22");
        assert_eq!(make(Direction::Out, Protocol::Udp, ""), "allow out 22/udp");
        assert_eq!(make(Direction::In, Protocol::Tcp, "1.2.3.4"), "allow in from 1.2.3.4 to any port 22 proto tcp");
        assert_eq!(make(Direction::Out, Protocol::Both, "1.2.3.4"), "allow out to 1.2.3.4 port 22");
    }

    #[test]
    fn a_preview_lands_in_the_right_columns() {
        let r = NewRule::from_form(Direction::Out, "53", Protocol::Udp, Action::Deny, "1.1.1.1").unwrap().preview();
        assert_eq!((r.direction, r.action.as_str(), r.port.as_str(), r.proto.as_str(), r.peer.as_str()), (Direction::Out, "deny", "53", "udp", "1.1.1.1"));
    }

    /// A queue with two removals, two additions and the power change.
    fn full_queue() -> Queue {
        let mut q = Queue::default();
        q.toggle_power(true);
        q.add(NewRule::from_form(Direction::In, "1", Protocol::Tcp, Action::Allow, "").unwrap());
        q.add(NewRule::from_form(Direction::In, "2", Protocol::Tcp, Action::Allow, "").unwrap());
        q.toggle_removed(&split_words("deny out 25/tcp"));
        q.toggle_removed(&split_words("route allow in on wg0 out on eth0"));
        q
    }

    #[test]
    fn the_order_is_removals_additions_then_power() {
        let got: Vec<String> = full_queue().ops().iter().map(|o| o.args.join(" ")).collect();
        assert_eq!(got, ["--force delete deny out 25/tcp", "--force route delete allow in on wg0 out on eth0", "allow in 1/tcp", "allow in 2/tcp", "--force disable"]);
    }

    #[test]
    fn a_partial_run_leaves_what_did_not_run() {
        for done in 0..=5 {
            let mut q = full_queue();
            let before = q.ops();
            q.drop_done(done);
            assert_eq!(q.ops(), before[done..], "after {done} commands");
        }
        let mut q = full_queue();
        q.drop_done(5);
        assert!(q.is_empty());
    }

    #[test]
    fn asking_again_takes_a_change_back() {
        let mut q = Queue::default();
        q.toggle_power(false);
        assert_eq!(q.power, Some(true));
        q.toggle_power(false);
        q.toggle_removed(&["a".to_string()]);
        q.toggle_removed(&["a".to_string()]);
        assert!(q.is_empty());
        let rule = NewRule::from_form(Direction::In, "1", Protocol::Tcp, Action::Allow, "").unwrap();
        q.add(rule.clone());
        q.add(rule);
        assert_eq!(q.added.len(), 1);
    }
}
