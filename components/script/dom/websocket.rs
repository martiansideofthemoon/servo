/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use dom::bindings::cell::DOMRefCell;
use dom::bindings::codegen::Bindings::EventHandlerBinding::EventHandlerNonNull;
use dom::bindings::codegen::Bindings::WebSocketBinding;
use dom::bindings::codegen::Bindings::WebSocketBinding::{BinaryType, WebSocketMethods};
use dom::bindings::codegen::InheritTypes::EventCast;
use dom::bindings::codegen::InheritTypes::EventTargetCast;
use dom::bindings::conversions::ToJSValConvertible;
use dom::bindings::error::{Error, Fallible};
use dom::bindings::global::{GlobalField, GlobalRef};
use dom::bindings::js::Root;
use dom::bindings::refcounted::Trusted;
use dom::bindings::str::USVString;
use dom::bindings::trace::JSTraceable;
use dom::bindings::utils::{Reflectable, reflect_dom_object};
use dom::blob::Blob;
use dom::closeevent::CloseEvent;
use dom::event::{Event, EventBubbles, EventCancelable};
use dom::eventtarget::EventTarget;
use dom::messageevent::MessageEvent;
use hyper::header::Host;
use js::jsapi::{JSAutoCompartment, JSAutoRequest, RootedValue};
use js::jsapi::{JS_GetArrayBufferData, JS_NewArrayBuffer};
use js::jsval::UndefinedValue;
use libc::{uint32_t, uint8_t};
use net_traits::hosts::replace_hosts;
use script_task::ScriptTaskEventCategory::WebSocketEvent;
use script_task::{CommonScriptMsg, Runnable};
use std::borrow::ToOwned;
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use std::{ptr, slice};
use util::str::DOMString;
use util::task::spawn_named;
use websocket::client::receiver::Receiver;
use websocket::client::request::Url;
use websocket::client::sender::Sender;
use websocket::header::Origin;
use websocket::message::CloseData;
use websocket::result::WebSocketResult;
use websocket::stream::WebSocketStream;
use websocket::ws::receiver::Receiver as WSReceiver;
use websocket::ws::sender::Sender as Sender_Object;
use websocket::ws::util::url::parse_url;
use websocket::{Client, Message};

#[derive(JSTraceable, PartialEq, Copy, Clone, Debug, HeapSizeOf)]
enum WebSocketRequestState {
    Connecting = 0,
    Open = 1,
    Closing = 2,
    Closed = 3,
}

no_jsmanaged_fields!(Sender<WebSocketStream>);

#[derive(HeapSizeOf)]
enum MessageData {
    Text(String),
    Binary(Vec<u8>),
}

// list of blacklist ports according to
// http://mxr.mozilla.org/mozilla-central/source/netwerk/base/nsIOService.cpp#87
const BLOCKED_PORTS_LIST: &'static [u16] = &[
    1,    // tcpmux
    7,    // echo
    9,    // discard
    11,   // systat
    13,   // daytime
    15,   // netstat
    17,   // qotd
    19,   // chargen
    20,   // ftp-data
    21,   // ftp-cntl
    22,   // ssh
    23,   // telnet
    25,   // smtp
    37,   // time
    42,   // name
    43,   // nicname
    53,   // domain
    77,   // priv-rjs
    79,   // finger
    87,   // ttylink
    95,   // supdup
    101,  // hostriame
    102,  // iso-tsap
    103,  // gppitnp
    104,  // acr-nema
    109,  // pop2
    110,  // pop3
    111,  // sunrpc
    113,  // auth
    115,  // sftp
    117,  // uucp-path
    119,  // nntp
    123,  // NTP
    135,  // loc-srv / epmap
    139,  // netbios
    143,  // imap2
    179,  // BGP
    389,  // ldap
    465,  // smtp+ssl
    512,  // print / exec
    513,  // login
    514,  // shell
    515,  // printer
    526,  // tempo
    530,  // courier
    531,  // Chat
    532,  // netnews
    540,  // uucp
    556,  // remotefs
    563,  // nntp+ssl
    587,  //
    601,  //
    636,  // ldap+ssl
    993,  // imap+ssl
    995,  // pop3+ssl
    2049, // nfs
    4045, // lockd
    6000, // x11
];

#[dom_struct]
pub struct WebSocket {
    eventtarget: EventTarget,
    url: Url,
    global: GlobalField,
    ready_state: Cell<WebSocketRequestState>,
    #[ignore_heap_size_of = "Defined in std"]
    sender: RefCell<Option<Arc<Mutex<Sender<WebSocketStream>>>>>,
    failed: Cell<bool>, //Flag to tell if websocket was closed due to failure
    full: Cell<bool>, //Flag to tell if websocket queue is full
    clean_close: Cell<bool>, //Flag to tell if the websocket closed cleanly (not due to full or fail)
    code: Cell<u16>, //Closing code
    reason: DOMRefCell<DOMString>, //Closing reason
    binary_type: Cell<BinaryType>,
}

/// *Establish a WebSocket Connection* as defined in RFC 6455.
fn establish_a_websocket_connection(resource_url: &Url, net_url: (Host, String, bool),
                                    origin: String)
   -> WebSocketResult<(Sender<WebSocketStream>, Receiver<WebSocketStream>)> {
    // URL that we actually fetch from the network, after applying the replacements
    // specified in the hosts file.

    let host = Host {
        hostname: resource_url.serialize_host().unwrap(),
        port: resource_url.port_or_default()
    };

    let mut request = try!(Client::connect(net_url));
    request.headers.set(Origin(origin));
    request.headers.set(host);

    let response = try!(request.send());
    try!(response.validate());

    Ok(response.begin().split())
}


impl WebSocket {
    fn new_inherited(global: GlobalRef, url: Url) -> WebSocket {
        WebSocket {
            eventtarget: EventTarget::new_inherited(),
            url: url,
            global: GlobalField::from_rooted(&global),
            ready_state: Cell::new(WebSocketRequestState::Connecting),
            failed: Cell::new(false),
            sender: RefCell::new(None),
            full: Cell::new(false),
            clean_close: Cell::new(true),
            code: Cell::new(0),
            reason: DOMRefCell::new("".to_owned()),
            binary_type: Cell::new(BinaryType::Blob),
        }

    }

    fn new(global: GlobalRef, url: Url) -> Root<WebSocket> {
        reflect_dom_object(box WebSocket::new_inherited(global, url),
                           global, WebSocketBinding::Wrap)
    }

    pub fn Constructor(global: GlobalRef,
                       url: DOMString,
                       protocols: Option<DOMString>)
                       -> Fallible<Root<WebSocket>> {
        // Step 1.
        let resource_url = try!(Url::parse(&url).map_err(|_| Error::Syntax));
        let net_url = try!(parse_url(&replace_hosts(&resource_url)).map_err(|_| Error::Syntax));

        // Step 2: Disallow https -> ws connections.

        // Step 3: Potentially block access to some ports.
        let port: u16 = resource_url.port_or_default().unwrap();

        if BLOCKED_PORTS_LIST.iter().any(|&p| p == port) {
            return Err(Error::Security);
        }

        // Step 4.
        let protocols: &[DOMString] = protocols
                                      .as_ref()
                                      .map_or(&[], |ref string| slice::ref_slice(string));

        // Step 5.
        for (i, protocol) in protocols.iter().enumerate() {
            // https://tools.ietf.org/html/rfc6455#section-4.1
            // Handshake requirements, step 10
            if protocol.is_empty() {
                return Err(Error::Syntax);
            }

            if protocols[i + 1..].iter().any(|p| p == protocol) {
                return Err(Error::Syntax);
            }

            if protocol.chars().any(|c| c < '\u{0021}' || c > '\u{007E}') {
                return Err(Error::Syntax);
            }
        }

        // Step 6: Origin.

        // Step 7.
        let ws = WebSocket::new(global, resource_url.clone());
        let address = Trusted::new(global.get_cx(), ws.r(), global.script_chan());

        let origin = global.get_url().serialize();
        let sender = global.script_chan();
        spawn_named(format!("WebSocket connection to {}", ws.Url()), move || {
            // Step 8: Protocols.

            // Step 9.
            let channel = establish_a_websocket_connection(&resource_url, net_url, origin);
            let (ws_sender, mut receiver) = match channel {
                Ok(channel) => channel,
                Err(e) => {
                    debug!("Failed to establish a WebSocket connection: {:?}", e);
                    let task = box CloseTask {
                        addr: address,
                    };
                    sender.send(CommonScriptMsg::RunnableMsg(WebSocketEvent, task)).unwrap();
                    return;
                }
            };
            let ws_sender = Arc::new(Mutex::new(ws_sender));

            let open_task = box ConnectionEstablishedTask {
                addr: address.clone(),
                sender: ws_sender.clone(),
            };
            sender.send(CommonScriptMsg::RunnableMsg(WebSocketEvent, open_task)).unwrap();

            for message in receiver.incoming_messages() {
                let message = match message {
                    Ok(Message::Text(text)) => MessageData::Text(text),
                    Ok(Message::Binary(data)) => MessageData::Binary(data),
                    Ok(Message::Ping(data)) => {
                        ws_sender.lock().unwrap().send_message(Message::Pong(data)).unwrap();
                        continue;
                    },
                    Ok(Message::Pong(_)) => continue,
                    Ok(Message::Close(data)) => {
                        ws_sender.lock().unwrap().send_message(Message::Close(data)).unwrap();
                        let task = box CloseTask {
                            addr: address,
                        };
                        sender.send(CommonScriptMsg::RunnableMsg(WebSocketEvent, task)).unwrap();
                        break;
                    },
                    Err(_) => break,
                };
                let message_task = box MessageReceivedTask {
                    address: address.clone(),
                    message: message,
                };
                sender.send(CommonScriptMsg::RunnableMsg(WebSocketEvent, message_task)).unwrap();
            }
        });

        // Step 7.
        Ok(ws)
    }
}

impl WebSocketMethods for WebSocket {
    // https://html.spec.whatwg.org/multipage/#handler-websocket-onopen
    event_handler!(open, GetOnopen, SetOnopen);

    // https://html.spec.whatwg.org/multipage/#handler-websocket-onclose
    event_handler!(close, GetOnclose, SetOnclose);

    // https://html.spec.whatwg.org/multipage/#handler-websocket-onerror
    event_handler!(error, GetOnerror, SetOnerror);

    // https://html.spec.whatwg.org/multipage/#handler-websocket-onmessage
    event_handler!(message, GetOnmessage, SetOnmessage);

    // https://html.spec.whatwg.org/multipage/#dom-websocket-url
    fn Url(&self) -> DOMString {
        self.url.serialize()
    }

    // https://html.spec.whatwg.org/multipage/#dom-websocket-readystate
    fn ReadyState(&self) -> u16 {
        self.ready_state.get() as u16
    }

    // https://html.spec.whatwg.org/multipage/#dom-websocket-binarytype
    fn BinaryType(&self) -> BinaryType {
        self.binary_type.get()
    }

    // https://html.spec.whatwg.org/multipage/#dom-websocket-binarytype
    fn SetBinaryType(&self, btype: BinaryType) {
        self.binary_type.set(btype)
    }

    // https://html.spec.whatwg.org/multipage/#dom-websocket-send
    fn Send(&self, data: USVString) -> Fallible<()> {
        match self.ready_state.get() {
            WebSocketRequestState::Connecting => {
                return Err(Error::InvalidState);
            },
            WebSocketRequestState::Open => (),
            WebSocketRequestState::Closing | WebSocketRequestState::Closed => {
                // TODO: Update bufferedAmount.
                return Ok(());
            }
        }

        /*TODO: This is not up to spec see http://html.spec.whatwg.org/multipage/comms.html search for
                "If argument is a string"
          TODO: Need to buffer data
          TODO: bufferedAmount attribute returns the size of the buffer in bytes -
                this is a required attribute defined in the websocket.webidl file
          TODO: The send function needs to flag when full by using the following
          self.full.set(true). This needs to be done when the buffer is full
        */
        let mut other_sender = self.sender.borrow_mut();
        let my_sender = other_sender.as_mut().unwrap();
        let _ = my_sender.lock().unwrap().send_message(Message::Text(data.0));
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/#dom-websocket-close
    fn Close(&self, code: Option<u16>, reason: Option<USVString>) -> Fallible<()>{
        fn send_close(this: &WebSocket) {
            this.ready_state.set(WebSocketRequestState::Closing);

            let mut sender = this.sender.borrow_mut();
            //TODO: Also check if the buffer is full
            if let Some(sender) = sender.as_mut() {
                let code: u16 = this.code.get();
                let reason = this.reason.borrow().clone();
                let _ = sender.lock().unwrap().send_message(Message::Close(Some(CloseData::new(code, reason))));
            }
        }


        if let Some(code) = code {
            //Check code is NOT 1000 NOR in the range of 3000-4999 (inclusive)
            if  code != 1000 && (code < 3000 || code > 4999) {
                return Err(Error::InvalidAccess);
            }
        }
        if let Some(ref reason) = reason {
            if reason.0.as_bytes().len() > 123 { //reason cannot be larger than 123 bytes
                return Err(Error::Syntax);
            }
        }

        match self.ready_state.get() {
            WebSocketRequestState::Closing | WebSocketRequestState::Closed  => {} //Do nothing
            WebSocketRequestState::Connecting => { //Connection is not yet established
                /*By setting the state to closing, the open function
                  will abort connecting the websocket*/
                self.failed.set(true);
                send_close(self);
                //Note: After sending the close message, the receive loop confirms a close message from the server and
                //      must fire a close event
            }
            WebSocketRequestState::Open => {
                //Closing handshake not started - still in open
                //Start the closing by setting the code and reason if they exist
                if let Some(code) = code {
                    self.code.set(code);
                }
                if let Some(reason) = reason {
                    *self.reason.borrow_mut() = reason.0;
                }
                send_close(self);
                //Note: After sending the close message, the receive loop confirms a close message from the server and
                //      must fire a close event
            }
        }
        Ok(()) //Return Ok
    }
}


/// Task queued when *the WebSocket connection is established*.
struct ConnectionEstablishedTask {
    addr: Trusted<WebSocket>,
    sender: Arc<Mutex<Sender<WebSocketStream>>>,
}

impl Runnable for ConnectionEstablishedTask {
    fn handler(self: Box<Self>) {
        let ws = self.addr.root();

        *ws.r().sender.borrow_mut() = Some(self.sender);

        // Step 1: Protocols.

        // Step 2.
        ws.ready_state.set(WebSocketRequestState::Open);

        // Step 3: Extensions.
        // Step 4: Protocols.
        // Step 5: Cookies.

        // Step 6.
        let global = ws.global.root();
        let event = Event::new(global.r(), "open".to_owned(),
                               EventBubbles::DoesNotBubble,
                               EventCancelable::NotCancelable);
        event.fire(EventTargetCast::from_ref(ws.r()));
    }
}

struct CloseTask {
    addr: Trusted<WebSocket>,
}

impl Runnable for CloseTask {
    fn handler(self: Box<Self>) {
        let ws = self.addr.root();
        let ws = ws.r();
        let global = ws.global.root();
        ws.ready_state.set(WebSocketRequestState::Closed);
        //If failed or full, fire error event
        if ws.failed.get() || ws.full.get() {
            ws.failed.set(false);
            ws.full.set(false);
            //A Bad close
            ws.clean_close.set(false);
            let event = Event::new(global.r(),
                                   "error".to_owned(),
                                   EventBubbles::DoesNotBubble,
                                   EventCancelable::Cancelable);
            let target = EventTargetCast::from_ref(ws);
            event.r().fire(target);
        }
        let rsn = ws.reason.borrow();
        let rsn_clone = rsn.clone();
        /*In addition, we also have to fire a close even if error event fired
         https://html.spec.whatwg.org/multipage/#closeWebSocket
        */
        let close_event = CloseEvent::new(global.r(),
                                          "close".to_owned(),
                                          EventBubbles::DoesNotBubble,
                                          EventCancelable::NotCancelable,
                                          ws.clean_close.get(),
                                          ws.code.get(),
                                          rsn_clone);
        let target = EventTargetCast::from_ref(ws);
        let event = EventCast::from_ref(close_event.r());
        event.fire(target);
    }
}

struct MessageReceivedTask {
    address: Trusted<WebSocket>,
    message: MessageData,
}

impl Runnable for MessageReceivedTask {
    #[allow(unsafe_code)]
    fn handler(self: Box<Self>) {
        let ws = self.address.root();
        debug!("MessageReceivedTask::handler({:p}): readyState={:?}", &*ws,
               ws.ready_state.get());

        // Step 1.
        if ws.ready_state.get() != WebSocketRequestState::Open {
            return;
        }

        // Step 2-5.
        let global = ws.global.root();
        let cx = global.r().get_cx();
        let _ar = JSAutoRequest::new(cx);
        let _ac = JSAutoCompartment::new(cx, ws.reflector().get_jsobject().get());
        let mut message = RootedValue::new(cx, UndefinedValue());
        match self.message {
            MessageData::Text(text) => text.to_jsval(cx, message.handle_mut()),
            MessageData::Binary(data) => {
                match ws.binary_type.get() {
                    BinaryType::Blob => {
                        let blob = Blob::new(global.r(), Some(data), "");
                        blob.to_jsval(cx, message.handle_mut());
                    }
                    BinaryType::Arraybuffer => {
                        unsafe {
                            let len = data.len() as uint32_t;
                            let buf = JS_NewArrayBuffer(cx, len);
                            let buf_data: *mut uint8_t = JS_GetArrayBufferData(buf, ptr::null());
                            ptr::copy_nonoverlapping(data.as_ptr(), buf_data, len as usize);
                            buf.to_jsval(cx, message.handle_mut());
                        }
                    }

                }
            },
        }

        let target = EventTargetCast::from_ref(ws.r());
        MessageEvent::dispatch_jsval(target, global.r(), message.handle());
    }
}
