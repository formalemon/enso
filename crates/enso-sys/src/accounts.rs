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

/// The users in the text of a passwd file: `name:password:uid:gid:…`.
pub fn parse_users(text: &str) -> Vec<User> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next().filter(|name| !name.is_empty())?.to_owned();
            let _password = fields.next()?;
            let uid = fields.next()?.parse().ok()?;
            let gid = fields.next()?.parse().ok()?;
            Some(User { name, uid, gid })
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
    fn this_machine_has_root() {
        assert_eq!(user_name(0).as_deref(), Some("root"));
        assert_eq!(group_name(0).as_deref(), Some("root"));
    }
}
