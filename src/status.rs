use imap_codec::imap_types::{
    core::Tag,
    mailbox::Mailbox,
    response::{Data, Response, Status},
    status::StatusDataItem,
};
use crate::api::MailboxId;
use serde_json::Value;

pub fn handle<'a>(tag: Tag<'a>, mailbox: Mailbox<'a>, mailbox_id: &MailboxId, folder: Value) -> Vec<Response<'a>> {
    let (existing_messages_count, unseen_messages_count) = match mailbox_id {
        MailboxId::Received(_) => (&folder["pagination"]["messagesRecusCount"], folder["pagination"]["messagesRecusNotReadCount"].as_u64()),
        MailboxId::Sent => (&folder["pagination"]["messagesEnvoyesCount"], None),
        MailboxId::Draft => (&folder["pagination"]["messagesDraftCount"], None),
        MailboxId::Archived => (&folder["pagination"]["messagesArchivesCount"], None),
    };
    let existing_messages_count = existing_messages_count.as_u64().unwrap() as u32;

    vec![
        Response::Data(Data::Status { mailbox: mailbox, items: vec![
            StatusDataItem::Messages(existing_messages_count),
            StatusDataItem::Unseen(unseen_messages_count.unwrap_or(0) as u32),
        ].into() }),
        Response::Status(Status::ok(Some(tag), None, "STATUS completed").unwrap()),
    ]
}
