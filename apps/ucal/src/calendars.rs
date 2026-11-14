//! Finding the calendar a person means by `--calendar`.

use calendar_core::{Account, Calendar};

/// The calendar called `query`: its id, its name (ignoring case) or
/// `Account/Name` when two accounts have a calendar of the same name.
pub fn find<'a>(
    query: &str,
    calendars: &'a [Calendar],
    accounts: &[Account],
) -> Result<&'a Calendar, String> {
    if let Some(c) = calendars.iter().find(|c| c.id == query) {
        return Ok(c);
    }
    let query = query.trim();
    let full_name = |c: &Calendar| format!("{}/{}", account_name(accounts, c), c.name);
    let by_full: Vec<_> = calendars
        .iter()
        .filter(|c| full_name(c).to_lowercase() == query.to_lowercase())
        .collect();
    if let [one] = by_full.as_slice() {
        return Ok(one);
    }
    let by_name: Vec<_> = calendars
        .iter()
        .filter(|c| c.name.to_lowercase() == query.to_lowercase())
        .collect();
    match by_name.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "There is no calendar called “{query}”. `ucal calendars` lists them."
        )),
        several => Err(format!(
            "Several calendars are called “{query}”; name one with its account: {}",
            several
                .iter()
                .map(|c| format!("“{}”", full_name(c)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The calendar new events go to: the desktop app's default calendar if it
/// can still take events, else the first one that can.
pub fn default<'a>(
    calendars: &'a [Calendar],
    setting: Option<&serde_json::Value>,
) -> Result<&'a Calendar, String> {
    let chosen = setting
        .and_then(|v| v.as_str())
        .and_then(|id| calendars.iter().find(|c| c.id == id && !c.read_only));
    chosen
        .or_else(|| calendars.iter().find(|c| !c.read_only))
        .ok_or_else(|| "No calendar can take new events.".to_owned())
}

pub fn account_name<'a>(accounts: &'a [Account], calendar: &Calendar) -> &'a str {
    accounts
        .iter()
        .find(|a| a.id == calendar.account_id)
        .map_or("", |a| a.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use calendar_core::ProviderKind;

    fn account(id: &str, name: &str) -> Account {
        Account {
            id: id.into(),
            name: name.into(),
            provider: ProviderKind::Caldav,
        }
    }

    fn calendar(id: &str, account: &str, name: &str, read_only: bool) -> Calendar {
        Calendar {
            id: id.into(),
            account_id: account.into(),
            name: name.into(),
            color: "#000000".into(),
            read_only,
        }
    }

    #[test]
    fn calendars_are_found_by_id_name_or_account_and_name() {
        let accounts = [account("a", "Home"), account("b", "Office")];
        let calendars = [
            calendar("1", "a", "Personal", false),
            calendar("2", "a", "Tasks", false),
            calendar("3", "b", "Tasks", false),
        ];
        let find = |q| find(q, &calendars, &accounts).map(|c| c.id.as_str());
        assert_eq!(find("personal"), Ok("1"));
        assert_eq!(find("3"), Ok("3"));
        assert_eq!(find("office/tasks"), Ok("3"));
        let several = find("Tasks").unwrap_err();
        assert!(several.contains("“Home/Tasks”") && several.contains("“Office/Tasks”"));
        assert!(find("Holidays").unwrap_err().contains("ucal calendars"));
    }

    #[test]
    fn new_events_go_to_the_default_calendar_if_it_is_writable() {
        let calendars = [
            calendar("holidays", "a", "Holidays", true),
            calendar("1", "a", "Personal", false),
            calendar("2", "a", "Work", false),
        ];
        let default =
            |v: Option<serde_json::Value>| default(&calendars, v.as_ref()).map(|c| c.id.clone());
        assert_eq!(default(Some("2".into())), Ok("2".into()));
        assert_eq!(default(Some("holidays".into())), Ok("1".into()));
        assert_eq!(default(None), Ok("1".into()));
        assert!(super::default(&calendars[..1], None).is_err());
    }
}
