use chrono::{Datelike, Local};
use imap_codec::imap_types::{
    flag::{Flag, FlagPerm},
    mailbox::Mailbox,
    response::{Code, Data, Response, Status},
};
use serde_json::Value;
use std::collections::HashMap;
use crate::api::MailboxId;

pub fn make_folders(folders: Vec<(String, u32)>) -> HashMap<String, MailboxId> {
    let mut map: HashMap<_, _> = folders.into_iter().map(|(name, id)| (name, MailboxId::Received(id))).collect();
    map.insert("INBOX".into(), MailboxId::Received(0));
    map.insert("Sent".into(), MailboxId::Sent);
    map.insert("Trash".into(), MailboxId::Archived);
    map.insert("Drafts".into(), MailboxId::Draft);
    map
}

pub fn filter<'a>(
    folders: &'a HashMap<String, MailboxId>,
    reference: Mailbox<'_>,
    mailbox_wildcard: &[u8],
) -> Vec<Response<'a>> {
    folders // TODO!!!
        .keys()
        .map(|folder| {
            Response::Data(Data::List {
                items: vec![],
                delimiter: None,
                mailbox: <Mailbox as TryFrom<&str>>::try_from(folder).unwrap(),
            })
        })
        .collect()
}

pub fn mailbox_info<'a>(mailbox_id: &MailboxId, folder: Value) -> Vec<Response<'a>> {
    let (existing_messages_count, unseen_messages_count) = match mailbox_id {
        MailboxId::Received(_) => (&folder["pagination"]["messagesRecusCount"], folder["pagination"]["messagesRecusNotReadCount"].as_u64()),
        MailboxId::Sent => (&folder["pagination"]["messagesEnvoyesCount"], None),
        MailboxId::Draft => (&folder["pagination"]["messagesDraftCount"], None),
        MailboxId::Archived => (&folder["pagination"]["messagesArchivesCount"], None),
    };
    let existing_messages_count = existing_messages_count.as_u64().unwrap() as u32;

    let date = Local::now().date_naive();
    // unwrap: Normalement on est après l'an 0
    let school_year: u32 = match date.month() {
        1..=8 => date.year() - 1,
        9..=12 => date.year(),
        _ => panic!("Month must be in the 1..=12 range"),
    }
    .try_into()
    .unwrap();

    let mut response = vec![
        Response::Data(Data::Flags(vec![Flag::Seen, Flag::Answered])),
        Response::Data(Data::Exists(existing_messages_count)),
        Response::Data(Data::Recent(0)),
        Response::Status(
            Status::ok(
                None,
                Some(Code::PermanentFlags(vec![FlagPerm::Flag(Flag::Seen)])),
                "Flags",
            )
            .unwrap(),
        ),
        Response::Status(
            Status::ok(
                None,
                Some(Code::UidValidity(school_year.try_into().unwrap())),
                format!("Valide en {}-{}", school_year, school_year + 1),
            )
            .unwrap(),
        ),
    ];

    if let Some(count) = unseen_messages_count {
        let count = count as u32;
        if let Ok(count) = count.try_into() {
            response.push(Response::Status(
                Status::ok(None, Some(Code::Unseen(count)), "Unseen").unwrap(),
            ));
        }
    }

    response
}
