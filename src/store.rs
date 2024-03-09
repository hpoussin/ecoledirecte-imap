use imap_codec::imap_types::{
    core::Tag,
    flag::{Flag, StoreResponse, StoreType},
    response::{Response, Status},
    sequence::{Sequence, SequenceSet, SeqOrUid},
};

pub fn handle<'a, F: Fn(&Vec<u32>, bool) -> ()>(tag: Tag<'a>, sequence_set: SequenceSet, kind: StoreType, response: StoreResponse, flags: Vec<Flag<'a>>, uid: bool, set_read_status: F) -> Vec<Response<'a>> {
    if !uid {
        vec![
            Response::Status(
                Status::no(Some(tag), None, "STORE Not supported (no UID)!").unwrap())
        ]
    } else if kind != StoreType::Add && kind != StoreType::Remove {
        vec![
            Response::Status(
                Status::no(Some(tag), None, "STORE Not supported (bad store type)!").unwrap())
        ]
    } else if flags != vec![Flag::Seen] {
        vec![
            Response::Status(
                Status::no(Some(tag), None, "STORE Not supported (bad flags)!").unwrap())
        ]
    } else {
        let message_ids = sequence_set.0
            .into_iter()
            .map(|sequence| match sequence {
                Sequence::Single(a) => match a {
                    SeqOrUid::Value(value) => value.into(),
                    SeqOrUid::Asterisk => panic!("STORE: invalid sequence range"),
                },
                Sequence::Range(_, _) => todo!(),
            })
            .collect::<Vec<_>>();
        set_read_status(&message_ids, kind == StoreType::Add);
        let mut responses = match response {
            StoreResponse::Silent => vec![],
            StoreResponse::Answer => message_ids
                .iter()
                .map(|_message_id| todo!())
                .collect(),
        };
        responses.push(Response::Status(Status::ok(Some(tag), None, "STORE completed").unwrap()));
        responses
    }
}
