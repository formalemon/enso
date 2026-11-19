//! Who's who: the names of users and groups, and their numbers, as
//! `/etc/passwd` and `/etc/group` have them.
//!
//! Accounts from a directory service or systemd-homed aren't in these
//! files; whoever asks has to have a fallback (the `USER` the session
//! was started with, say, or the bare number).

use std::fs;

/// A line of `/etc/passwd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
    /// The primary group.
    pub gid: u32,
    /// The comment field's first part: the person's full name, if given.
    pub full_name: String,
    pub home: String,
    /// The login shell; empty means `/bin/sh`.
    pub shell: String,
}

impl User {
    /// What to call them: the full name, or the login name without one.
    pub fn display_name(&self) -> &str {
        if self.full_name.is_empty() {
            &self.name
        } else {
            &self.full_name
        }
    }

    /// The login shell, with the empty field's meaning spelled out.
    pub fn login_shell(&self) -> &str {
        if self.shell.is_empty() {
            "/bin/sh"
        } else {
            &self.shell
        }
    }
}

/// A line of `/etc/group`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub gid: u32,
    /// Those it's a supplementary group of, by name. (Those it's the
    /// primary group of are known by their [`User::gid`].)
    pub members: Vec<String>,
}

/// The users in `/etc/passwd`.
pub fn users() -> Vec<User> {
    parse_users(&fs::read_to_string("/etc/passwd").unwrap_or_default())
}

/// The groups in `/etc/group`.
pub fn groups() -> Vec<Group> {
    parse_groups(&fs::read_to_string("/etc/group").unwrap_or_default())
}

/// The people who can log in here, in `/etc/passwd`'s order: see
/// [`login_users_in`].
pub fn login_users() -> Vec<User> {
    let read = |path| fs::read_to_string(path).unwrap_or_default();
    login_users_in(
        &read("/etc/passwd"),
        &read("/etc/shells"),
        &read("/etc/login.defs"),
    )
}

/// Of the users in `passwd`, those who are people and can log in: a
/// number in the range `login.defs` gives ordinary users (`UID_MIN` to
/// `UID_MAX`, 1000 to 60000 unless it says otherwise), and a shell that
/// `shells` lists. `nologin` and `false` are never shells, listed or not.
pub fn login_users_in(passwd: &str, shells: &str, login_defs: &str) -> Vec<User> {
    let setting = |key: &str, default: u32| {
        login_defs
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some(key)).then(|| words.next()?.parse().ok())?
            })
            .next_back()
            .unwrap_or(default)
    };
    let (min, max) = (setting("UID_MIN", 1000), setting("UID_MAX", 60000));
    let listed: Vec<&str> = shells
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/'))
        .collect();
    parse_users(passwd)
        .into_iter()
        .filter(|user| (min..=max).contains(&user.uid))
        .filter(|user| {
            let shell = user.login_shell();
            let refuses = shell.ends_with("/nologin") || shell.ends_with("/false");
            !refuses && listed.contains(&shell)
        })
        .collect()
}

/// The name of the user numbered `uid`.
pub fn user_name(uid: u32) -> Option<String> {
    users()
        .into_iter()
        .find(|user| user.uid == uid)
        .map(|user| user.name)
}

/// The name of the group numbered `gid`.
pub fn group_name(gid: u32) -> Option<String> {
    groups()
        .into_iter()
        .find(|group| group.gid == gid)
        .map(|group| group.name)
}

/// The users in the text of a passwd file:
/// `name:password:uid:gid:comment:home:shell`.
pub fn parse_users(text: &str) -> Vec<User> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next().filter(|name| !name.is_empty())?.to_owned();
            let _password = fields.next()?;
            let uid = fields.next()?.parse().ok()?;
            let gid = fields.next()?.parse().ok()?;
            // The comment is `full name,room,phone,…` by old convention.
            let full_name = fields
                .next()
                .unwrap_or_default()
                .split(',')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
            let home = fields.next().unwrap_or_default().to_owned();
            let shell = fields.next().unwrap_or_default().to_owned();
            Some(User {
                name,
                uid,
                gid,
                full_name,
                home,
                shell,
            })
        })
        .collect()
}

/// The groups in the text of a group file: `name:password:gid:members`.
pub fn parse_groups(text: &str) -> Vec<Group> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next().filter(|name| !name.is_empty())?.to_owned();
            let _password = fields.next()?;
            let gid = fields.next()?.parse().ok()?;
            let members = fields
                .next()
                .unwrap_or_default()
                .split(',')
                .filter(|member| !member.is_empty())
                .map(str::to_owned)
                .collect();
            Some(Group { name, gid, members })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lines_and_skips_what_isnt_one() {
        let users = parse_users(
            "root:x:0:0::/root:/bin/bash\n# a comment\nkai:x:1000:1000::/home/kai:/bin/bash\n:x:7:7\n",
        );
        let names: Vec<_> = users.iter().map(|user| (&*user.name, user.uid)).collect();
        assert_eq!(names, [("root", 0), ("kai", 1000)]);
        let groups = parse_groups("wheel:x:998:kai,root\nren:x:1000:\nbroken\n");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].members, ["kai", "root"]);
        assert!(groups[1].members.is_empty());
    }

    #[test]
    fn people_who_can_log_in() {
        let passwd = "\
root:x:0:0::/root:/bin/bash
nobody:x:65534:65534:Kernel Overflow User:/:/usr/bin/nologin
greeter:x:966:966:greetd greeter user:/:/bin/bash
kai:x:1000:1000:Kai Lin,,,:/home/kai:/bin/bash
mo:x:1001:1001::/home/mo:/usr/bin/fish
gone:x:1002:1002::/home/gone:/usr/bin/nologin
odd:x:1003:1003::/home/odd:/opt/unlisted
plain:x:1004:1004::/home/plain:
";
        let shells = "# comment\n/bin/sh\n/bin/bash\n/usr/bin/fish\n/usr/bin/nologin\n";
        let users = login_users_in(passwd, shells, "");
        let names: Vec<_> = users.iter().map(|user| user.name.as_str()).collect();
        assert_eq!(names, ["kai", "mo", "plain"]);
        assert_eq!(users[0].display_name(), "Kai Lin");
        assert_eq!(users[0].home, "/home/kai");
        assert_eq!(users[1].display_name(), "mo");
        assert_eq!(users[2].login_shell(), "/bin/sh");

        // login.defs moves the range; the last setting wins.
        let defs = "UID_MIN 500\n#UID_MIN 2000\nUID_MAX 1000\nUID_MAX\t1000\n";
        let names: Vec<_> = login_users_in(passwd, shells, defs)
            .into_iter()
            .map(|user| user.name)
            .collect();
        assert_eq!(names, ["greeter", "kai"]);
    }

    #[test]
    fn this_machine_has_root() {
        assert_eq!(user_name(0).as_deref(), Some("root"));
        assert_eq!(group_name(0).as_deref(), Some("root"));
    }
}
