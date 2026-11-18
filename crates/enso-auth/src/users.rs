//! Who may answer a polkit request: polkit names users and groups by
//! number; the helper wants a user's name. Names come from `/etc/passwd`
//! and `/etc/group`.

use enso_sys::accounts::{self, Group, User};

/// Someone polkit would take the password of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    User(u32),
    /// Any member of the group (`wheel`, for administrators on Arch).
    Group(u32),
}

/// The users who may answer, by name: the person at the screen (`me`)
/// first, root last, groups opened up into their members.
pub fn names(identities: &[Identity], me: u32) -> Vec<String> {
    resolve(identities, me, &accounts::users(), &accounts::groups())
}

fn resolve(identities: &[Identity], me: u32, accounts: &[User], groups: &[Group]) -> Vec<String> {
    let mut uids: Vec<u32> = Vec::new();
    for identity in identities {
        match *identity {
            Identity::User(uid) => uids.push(uid),
            Identity::Group(gid) => {
                // Its members by name, and those it is the primary group of.
                let members = groups
                    .iter()
                    .find(|group| group.gid == gid)
                    .map(|group| group.members.as_slice())
                    .unwrap_or_default();
                for member in members {
                    if let Some(account) = accounts.iter().find(|account| &account.name == member) {
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

    fn users() -> Vec<User> {
        accounts::parse_users(PASSWD)
    }

    fn groups() -> Vec<Group> {
        accounts::parse_groups(GROUP)
    }

    #[test]
    fn groups_open_up_and_the_person_here_comes_first() {
        let wheel = [Identity::Group(998)];
        assert_eq!(
            resolve(&wheel, 1000, &users(), &groups()),
            ["ren", "admin", "root"]
        );
        // A guest at the screen isn't in wheel, so isn't offered: an
        // administrator has to answer.
        let names = resolve(&wheel, 1001, &users(), &groups());
        assert_eq!(names, ["ren", "admin", "root"]);
        assert!(!names.iter().any(|name| name == "guest"));
        let wanted = [
            Identity::User(0),
            Identity::User(1000),
            Identity::User(1000),
        ];
        assert_eq!(resolve(&wanted, 1000, &users(), &groups()), ["ren", "root"]);
        assert!(resolve(&[Identity::User(4242)], 1000, &users(), &groups()).is_empty());
    }
}
