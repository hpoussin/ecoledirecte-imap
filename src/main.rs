use imap_codec::{
    decode::{AuthenticateDataDecodeError, CommandDecodeError, Decoder},
    encode::Encoder,
    imap_types::{
        self,
        auth::AuthMechanism,
        bounded_static::IntoBoundedStatic,
        command::Command,
        core::Text,
        mailbox::{ListMailbox, Mailbox},
        response::{
            Code, CommandContinuationRequest, Data, Greeting, GreetingKind, Response, Status,
        },
        secret::Secret,
        state::State,
    },
    AuthenticateDataCodec, CommandCodec, GreetingCodec, ResponseCodec,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::ops::Range;
use std::str;
use std::thread;

use ecoledirecte_imap::api;
use ecoledirecte_imap::auth;
use ecoledirecte_imap::capabilities;
use ecoledirecte_imap::fetch;
use ecoledirecte_imap::lsub;
use ecoledirecte_imap::mailbox;
use ecoledirecte_imap::status;
use ecoledirecte_imap::store;
use api::MailboxId;

struct Connection<'a> {
    state: State<'a>,
    user: Option<auth::User>,
    folders: Option<HashMap<String, MailboxId>>,
}

impl<'a> Default for Connection<'a> {
    fn default() -> Connection<'a> {
        Connection {
            state: State::Greeting,
            user: None,
            folders: None,
        }
    }
}

fn main() {
    let listener = TcpListener::bind("localhost:1993").unwrap();
    let client = reqwest::blocking::Client::new();

    thread::scope(|s| {
        for stream in listener.incoming() {
            let stream = stream.unwrap();

            s.spawn(|| responder(stream, Connection::default(), &client));
        }
    });
}

trait AsRange {
    fn as_range_of(&self, other: &Self) -> Option<Range<usize>>;
}

impl<T> AsRange for [T] {
    fn as_range_of(&self, other: &[T]) -> Option<Range<usize>> {
        let self_ = self.as_ptr_range();
        let other = other.as_ptr_range();
        if other.start > self_.start || self_.end > other.end {
            None
        } else {
            let from = unsafe { self_.start.offset_from(other.start) };
            let to = unsafe { self_.end.offset_from(other.start) };
            Some((from as usize)..(to as usize))
        }
    }
}

fn responder(
    mut stream: TcpStream,
    mut connection: Connection<'_>,
    client: &reqwest::blocking::Client,
) {
    let mut buffer = [0u8; 1024];
    let mut cursor = 0;

    stream
        .write(
            &GreetingCodec::default()
                .encode(&Greeting {
                    kind: GreetingKind::Ok,
                    code: Some(Code::Capability(capabilities())),
                    text: Text::try_from("ecoledirecte-imap ready").unwrap(),
                })
                .dump(),
        )
        .unwrap();

    connection.state = State::NotAuthenticated;

    loop {
        match CommandCodec::default().decode(&buffer[..cursor]) {
            Ok((remaining, command)) => {
                print!(
                    "C: {}",
                    str::from_utf8(&CommandCodec::default().encode(&command).dump()).unwrap()
                );
                for response in process(command, &mut connection, &mut stream, client) {
                    print!(
                        "S: {}",
                        str::from_utf8(&ResponseCodec::default().encode(&response).dump()).unwrap()
                    );
                    stream
                        .write(&ResponseCodec::default().encode(&response).dump())
                        .unwrap();
                }

                if let State::Logout = connection.state {
                    break;
                }

                let range = remaining.as_range_of(&buffer).unwrap();
                cursor = range.len();
                buffer.copy_within(range, 0);
            }
            Err(CommandDecodeError::LiteralFound { tag, length, mode }) => {
                todo!("LITERAL {:?} {} {:?}", tag, length, mode)
            }
            Err(CommandDecodeError::Incomplete) => {
                if cursor >= buffer.len() {
                    todo!("OUT OF MEMORY!");
                }
                let received = stream.read(&mut buffer[cursor..]).unwrap();
                if received == 0 {
                    break;
                }
                cursor += received;
            }
            Err(CommandDecodeError::Failed) => {
                stream
                    .write(
                        &ResponseCodec::default()
                            .encode(&Response::Status(
                                Status::bad(None, None, "Parsing failed").unwrap(),
                            ))
                            .dump(),
                    )
                    .unwrap();
                cursor = 0;
            }
        }
    }
}

fn process<'a>(
    command: Command<'a>,
    connection: &'a mut Connection<'_>,
    stream: &mut TcpStream,
    client: &reqwest::blocking::Client,
) -> Vec<Response<'a>> {
    use imap_types::{
        command::CommandBody::*,
        command::CommandBody::{Logout, Status as StatusCommand},
        response::Status,
        state::State::{Logout as LogoutState, *},
    };

    // Déplacement partiel (tag reste possédé par command)
    match command.body {
        Capability => {
            return vec![
                Response::Data(Data::Capability(capabilities())),
                Response::Status(
                    Status::ok(Some(command.tag), None, "CAPABILITY completed").unwrap(),
                ),
            ]
        }
        Noop => {
            return vec![Response::Status(
                Status::ok(Some(command.tag), None, "NOOP completed").unwrap(),
            )]
        }
        Logout => {
            connection.state = LogoutState;
            return vec![
                Response::Status(Status::bye(None, "Logging out!").unwrap()),
                Response::Status(Status::ok(Some(command.tag), None, "LOGOUT completed").unwrap()),
            ];
        }
        _ => (),
    }

    if connection.state == NotAuthenticated {
        match command.body {
            Authenticate {
                mechanism,
                initial_response,
            } => {
                if mechanism != AuthMechanism::Plain {
                    return vec![Response::Status(
                        Status::no(Some(command.tag), None, "Unsupported mechanism").unwrap(),
                    )];
                }
                if initial_response != None {
                    return vec![Response::Status(
                        Status::no(Some(command.tag), None, "Unexpected initial response").unwrap(),
                    )];
                }

                stream
                    .write(
                        &ResponseCodec::default()
                            .encode(&Response::CommandContinuationRequest(
                                CommandContinuationRequest::Base64(Cow::Borrowed(&[])),
                            ))
                            .dump(),
                    )
                    .unwrap();

                // TODO: Malgré cette tentative de faire marcher les choses (voir commentaire suivant),
                // ça va pas :
                // Si le client envoie un paquet AAABBB avec AAA une commande AUTHENTICATE PLAIN
                // normalement on devrait lire BBB pour avoir les données d'authentification
                // mais ici on n'y a pas accès (elles sont dans le buffer de la boucle principale)
                // donc on se retrouvera à lire CCC d'un autre paquet
                // Donc la gestion totale pour AAABBB, CCC... serait AAA, CCC, BBB, ...
                // ce qui ne va clairement pas (même si en pratique si le client est bien discipliné
                // il devrait attendre de recevoir la confirmation du serveur pour envoyer les données
                // d'authentification. il y a quand même de quoi améliorer les choses + aussi les
                // littéraux non-synchronisants poseraient problème (mais je sais pas s'ils peuvent
                // être utilisés pendant l'authentification))
                let mut buffer = [0u8; 1024];
                let mut consumed = 0;
                let mut peeked;

                peeked = stream.peek(&mut buffer).unwrap();

                // Le problème est de consommer juste la bonne quantité de données
                // pour que le reste soit géré par la boucle principale
                // On pourrait utiliser juste peek puis consommer ce qui a été con-
                // sommé par le codec mais le problème est que peek ne bloque pas
                // et donc on attendrait en boucle quand il manque des données.
                // La solution: peek pour obtenir des données (puisque quand il n'y
                // a pas de données disponibles peek bloque) puis si c'est pas suf-
                // fisant, on read() les données pour les consommer (puisqu'on sait
                // qu'on les utilise de toute manière) et on peek à nouveau.
                let line = loop {
                    match AuthenticateDataCodec::default().decode(&buffer[..peeked]) {
                        Ok((remaining, line)) => {
                            // unwrap: ok puisque remaining est une slice de buffer
                            let range = remaining.as_range_of(&buffer).unwrap();
                            // unwrap: ok puisque déjà peeked
                            stream.read(&mut buffer[consumed..range.start]).unwrap();
                            break line;
                        }
                        Err(AuthenticateDataDecodeError::Incomplete) => {
                            if peeked >= buffer.len() {
                                todo!("OUT OF MEMORY");
                            }
                            // unwrap: ok puisque déjà peeked
                            stream.read(&mut buffer[consumed..peeked]).unwrap();
                            consumed = peeked;
                            let received = stream.peek(&mut buffer[consumed..]).unwrap();
                            if received == 0 {
                                return vec![];
                            }
                            peeked += received;
                        }
                        Err(AuthenticateDataDecodeError::Failed) => {
                            stream.read(&mut buffer[consumed..peeked]).unwrap();
                            return vec![Response::Status(
                                Status::bad(Some(command.tag), None, "Invalid BASE64 literal")
                                    .unwrap(),
                            )];
                        }
                    }
                };

                /* AuthenticateDataCodec ne gère par "*" mais l'erreur failed le gère
                 * (pour la mauvaise raison :p)
                 */
                let (username, password) = match auth::parse_plain_message(
                    Secret::new(&line.0.declassify()),
                    command.tag.clone(),
                ) {
                    Ok(tup) => tup,
                    Err(response) => {
                        return response;
                    }
                };

                let (state, user, response) =
                    auth::translate(api::login(client, username, password), command.tag);

                connection.state = state;
                connection.user = user;
                return response;
            }
            Login { username, password } => {
                let (state, user, response) = auth::translate(
                    api::login(
                        client,
                        str::from_utf8(username.as_ref()).unwrap(),
                        str::from_utf8(password.declassify().as_ref()).unwrap(),
                    ),
                    command.tag,
                );

                connection.state = state;
                connection.user = user;
                return response;
            }
            _ => (),
        }
    }

    if let Authenticated | Selected(_) = connection.state {
        match command.body {
            Select { mailbox } => {
                // unwrap: on est en authenticated ou selected
                let user = connection.user.as_ref().unwrap();
                if let None = connection.folders {
                    let folders =
                        mailbox::make_folders(api::get_folders(client, user.id, &user.token));
                    connection.folders = Some(folders);
                }
                let folders = connection.folders.as_ref().unwrap();

                let name = match mailbox {
                    Mailbox::Inbox => "INBOX",
                    Mailbox::Other(ref mailbox) => str::from_utf8(mailbox.as_ref()).unwrap(),
                };
                match folders.get(name) {
                    Some(&ref mailbox_id) => {
                        let mut response = mailbox::mailbox_info(
                            mailbox_id,
                            api::get_folder_info(client, mailbox_id, user.id, &user.token),
                        );
                        response.push(Response::Status(
                            Status::ok(
                                Some(command.tag),
                                Some(Code::ReadWrite),
                                "SELECT completed",
                            )
                            .unwrap(),
                        ));

                        connection.state = State::Selected(mailbox.into_static());
                        return response;
                    }
                    None => {
                        let folders =
                            mailbox::make_folders(api::get_folders(client, user.id, &user.token));
                        connection.folders = Some(folders);
                        let folders = connection.folders.as_ref().unwrap();
                        if folders.contains_key(name) {
                            return process(
                                Command {
                                    tag: command.tag,
                                    body: Select { mailbox },
                                },
                                connection,
                                stream,
                                client,
                            );
                        } else {
                            return vec![Response::Status(
                                Status::no(Some(command.tag), None, "No such mailbox!").unwrap(),
                            )];
                        }
                    }
                }
            }
            Examine { mailbox } => todo!("EXAMINE {:?}", mailbox),
            Create { mailbox } => todo!("CREATE {:?}", mailbox),
            Delete { mailbox } => todo!("DELETE {:?}", mailbox),
            Rename { from, to } => todo!("RENAME {:?} {:?}", from, to),
            Lsub {
                reference,
                mailbox_wildcard
            } => {
                return lsub::handle(command.tag, reference, mailbox_wildcard);
            },
            List {
                reference,
                mailbox_wildcard,
            } => {
                use imap_types::flag::FlagNameAttribute::Noselect;
                let name = match mailbox_wildcard {
                    ListMailbox::String(ref name) => name.as_ref(),
                    ListMailbox::Token(ref name) => name.as_ref(),
                };

                if name.len() == 0 {
                    return vec![
                        Response::Data(Data::List {
                            items: vec![Noselect],
                            delimiter: None,
                            mailbox: Mailbox::try_from("").unwrap(),
                        }),
                        Response::Status(
                            Status::ok(Some(command.tag), None, "LIST completed").unwrap(),
                        ),
                    ];
                }

                // unwrap: on est en authenticated ou selected
                let user = connection.user.as_ref().unwrap();
                connection.folders = Some(mailbox::make_folders(api::get_folders(
                    client,
                    user.id,
                    &user.token,
                )));
                let folders = connection.folders.as_ref().unwrap();

                let mut response = mailbox::filter(folders, reference, name);

                response.push(Response::Status(
                    Status::ok(Some(command.tag), None, "LIST completed").unwrap(),
                ));
                return response;
            }
            StatusCommand {
                mailbox,
                item_names: _,
            } => {
                let user = connection.user.as_ref().unwrap();
                let mailbox_id = connection.folders.as_ref().unwrap().get(match mailbox {
                    Mailbox::Inbox => "INBOX",
                    Mailbox::Other(ref mailbox) => str::from_utf8(mailbox.as_ref()).unwrap(),
                });
                return match mailbox_id {
                    Some(mailbox_id) => status::handle(
                        command.tag,
                        mailbox,
                        mailbox_id,
                        api::get_folder_info(client, mailbox_id, user.id, &user.token)),
                    None => vec![
                        Response::Status(
                            Status::no(Some(command.tag), None, "STATUS No such mailbox!").unwrap())
                    ],
                }
            },
            Store {
                sequence_set,
                kind,
                response,
                flags,
                uid,
            } => {
                let user = connection.user.as_ref().unwrap();
                return store::handle(
                    command.tag,
                    sequence_set,
                    kind,
                    response,
                    flags,
                    uid,
                    |message_ids, read_status| api::set_read_status(client, user.id, &user.token, read_status, message_ids));
            },
            _ => (),
        }
    }

    if let Selected(mailbox) = &connection.state {
        match command.body {
            Check => todo!("CHECK ({:?})", mailbox),
            Close => {
                connection.state = State::Authenticated;
                return vec![Response::Status(
                    Status::ok(Some(command.tag), None, "Mailbox closed").unwrap(),
                )];
            }
            Search {
                charset,
                criteria,
                uid,
            } => todo!(
                "SEARCH {:?} {:?} {:?} ({:?})",
                charset,
                criteria,
                uid,
                mailbox
            ),
            Fetch {
                sequence_set,
                macro_or_item_names,
                uid,
            } => {
                let user = connection.user.as_ref().unwrap();
                let mailbox_id = connection.folders.as_ref().unwrap().get(match mailbox {
                    Mailbox::Inbox => "INBOX",
                    Mailbox::Other(ref mailbox) => str::from_utf8(mailbox.as_ref()).unwrap(),
                }).unwrap();
                let messages = api::get_messages(client, user.id, &user.token, mailbox_id);
                return fetch::handle(
                    command.tag,
                    sequence_set,
                    macro_or_item_names,
                    uid,
                    messages,
                    |message_id| api::get_message(client, user.id, &user.token, &mailbox_id, message_id),
                    |attachment_id| api::get_attachment(client, &user.token, attachment_id));
            }
            _ => (),
        }
    }

    vec![Response::Status(
        Status::no(Some(command.tag), None, "Not supported!").unwrap(),
    )]
}
