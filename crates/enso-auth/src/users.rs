//! Who may answer a polkit request: polkit names users and groups by
//! number; the helper wants a user's name. Names come from `/etc/passwd`
//! and `/etc/group`.

use std::fs;

/// Someone polkit would take the password of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    User(u32),
    /// Any member of the group (`wheel`, for administrators on Arch).
    Group(u32),
}

/// A line of `/etc/passwd`: name, uid, primary gid.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Account {
    name: String,
    uid: u32,
    gid: u32,
}

/// The users who may answer, by name: the person at the screen (`me`)
/// first, root last, groups opened up into their members.
pub fn names(identities: &[Identity], me: u32) -> Vec<String> {
    let passwd = fs::read_to_string("/etc/passwd").unwrap_or_default();
    let group = fs::read_to_string("/etc/group").unwrap_or_default();
    resolve(identities, me, &passwd, &group)
}

fn resolve(identities: &[Identity], me: u32, passwd: &str, group: &str) -> Vec<String> {
    let accounts: Vec<Account> = passwd.lines().filter_map(account).collect();
    let mut uids: Vec<u32> = Vec::new();
    for identity in identities {
        match *identity {
            Identity::User(uid) => uids.push(uid),
            Identity::Group(gid) => {
                // Its members by name, and those it is the primary group of.
                let members = group
                    .lines()
                    .map(|line| line.split(':').collect::<Vec<_>>())
                    .find(|fields| fields.len() >= 4 && fields[2].parse() == Ok(gid))
                    .map(|fields| fields[3].to_owned())
                    .unwrap_or_default();
                for member in members.split(',').filter(|name| !name.is_empty()) {
                    if let Some(account) = accounts.iter().find(|account| account.name == member) {
                        uids.push(account.uid);
                    }
                }
                uids.extend(accounts.iter().filter(|a| a.gid == gid).map(|a| a.uid));
            }
        }
    }
    uids.sort_by_key(|&uid| (uid != me, uid == 0, uid));
    uids.dedup();
    uids.into_iter()
        .filter_map(|uid| {
            accounts
                .iter()
                .find(|account| account.uid == uid)
                .map(|account| account.name.clone())
                // Not in /etc/passwd (a systemd-homed user, say): only the
                // person at the screen has a name to hand, from the session.
                .or_else(|| (uid == me).then(|| std::env::var("USER").ok()).flatten())
        })
        .collect()
}

fn account(line: &str) -> Option<Account> {
    let mut fields = line.split(':');
    let name = fields.next()?.to_owned();
    let _password = fields.next()?;
    let uid = fields.next()?.parse().ok()?;
    let gid = fields.next()?.parse().ok()?;
    Some(Account { name, uid, gid })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0::/root:/bin/bash
ren:x:1000:1000::/home/ren:/bin/bash
guest:x:1001:1001::/home/guest:/bin/bash
admin:x:1002:998::/home/admin:/bin/bash
";
    const GROUP: &str = "root:x:0:root
wheel:x:998:ren,root
ren:x:1000:
";

    #[test]
    fn groups_open_up_and_the_person_here_comes_first() {
        let wheel = [Identity::Group(998)];
        assert_eq!(
            resolve(&wheel, 1000, PASSWD, GROUP),
            ["ren", "admin", "root"]
        );
        // A guest at the screen isn't in wheel, so isn't offered: an
        // administrator has to answer.
        let names = resolve(&wheel, 1001, PASSWD, GROUP);
        assert_eq!(names, ["ren", "admin", "root"]);
        assert!(!names.iter().any(|name| name == "guest"));
        let users = [
            Identity::User(0),
            Identity::User(1000),
            Identity::User(1000),
        ];
        assert_eq!(resolve(&users, 1000, PASSWD, GROUP), ["ren", "root"]);
        assert!(resolve(&[Identity::User(4242)], 1000, PASSWD, GROUP).is_empty());
    }
}
