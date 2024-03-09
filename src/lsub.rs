use imap_codec::imap_types::{
    core::Tag,
    mailbox::{ListMailbox, Mailbox},
    response::{Response, Status},
};

pub fn handle<'a>(tag: Tag<'a>, _reference: Mailbox, _mailbox_wildcard: ListMailbox) -> Vec<Response<'a>> {
    vec!(Response::Status(
        Status::ok(Some(tag), None, "LSUB completed").unwrap(),
    ))
}
