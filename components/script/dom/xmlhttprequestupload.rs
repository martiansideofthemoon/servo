/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use dom::bindings::codegen::Bindings::XMLHttpRequestUploadBinding;
use dom::bindings::global::GlobalRef;
use dom::bindings::js::Root;
use dom::bindings::utils::reflect_dom_object;
use dom::xmlhttprequesteventtarget::XMLHttpRequestEventTarget;

#[dom_struct]
pub struct XMLHttpRequestUpload {
    eventtarget: XMLHttpRequestEventTarget
}

impl XMLHttpRequestUpload {
    fn new_inherited() -> XMLHttpRequestUpload {
        XMLHttpRequestUpload {
            eventtarget: XMLHttpRequestEventTarget::new_inherited(),
        }
    }
    pub fn new(global: GlobalRef) -> Root<XMLHttpRequestUpload> {
        reflect_dom_object(box XMLHttpRequestUpload::new_inherited(),
                           global,
                           XMLHttpRequestUploadBinding::Wrap)
    }
}
