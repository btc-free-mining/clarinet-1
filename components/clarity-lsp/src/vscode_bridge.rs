extern crate console_error_panic_hook;
use crate::backend::{process_notification, process_request, LspNotification, LspRequest};
use crate::state::EditorState;
use crate::utils::{
    clarity_diagnostics_to_lsp_type, get_contract_location, get_manifest_location, log,
};
use clarinet_files::{FileAccessor, WASMFileSystemAccessor};
use js_sys::{Function as JsFunction, Promise};
use lsp_types::{
    notification::{DidOpenTextDocument, DidSaveTextDocument, Initialized, Notification},
    request::{Completion, Request},
    CompletionParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    PublishDiagnosticsParams, ShowMessageParams, Url,
};
use serde_wasm_bindgen::{from_value as decode_from_js, to_value as encode_to_js};
use std::{
    panic,
    sync::{Arc, RwLock},
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

#[wasm_bindgen]
pub struct LspVscodeBridge {
    editor_state_lock: Arc<RwLock<EditorState>>,
    client_diagnostic_tx: JsFunction,
    client_notification_tx: JsFunction,
    backend_to_client_tx: JsFunction,
}

#[wasm_bindgen]
impl LspVscodeBridge {
    #[wasm_bindgen(constructor)]
    pub fn new(
        client_diagnostic_tx: JsFunction,
        client_notification_tx: JsFunction,
        backend_to_client_tx: JsFunction,
    ) -> LspVscodeBridge {
        panic::set_hook(Box::new(console_error_panic_hook::hook));

        let editor_state_lock = Arc::new(RwLock::new(EditorState::new()));
        LspVscodeBridge {
            editor_state_lock,
            client_diagnostic_tx,
            client_notification_tx,
            backend_to_client_tx,
        }
    }

    #[wasm_bindgen(js_name=onNotification)]
    pub fn notification_handler(&self, method: String, js_params: JsValue) -> Promise {
        let file_accessor: Box<dyn FileAccessor> = Box::new(WASMFileSystemAccessor::new(
            self.backend_to_client_tx.clone(),
        ));

        match method.as_str() {
            Initialized::METHOD => {}

            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams = match decode_from_js(js_params) {
                    Ok(params) => params,
                    Err(err) => return Promise::reject(&JsValue::from(format!("error: {}", err))),
                };
                let uri = params.text_document.uri;
                log!("> opened: {}", &uri);

                let command = if let Some(contract_location) = get_contract_location(&uri) {
                    LspNotification::ContractOpened(contract_location)
                } else if let Some(manifest_location) = get_manifest_location(&uri) {
                    LspNotification::ManifestOpened(manifest_location)
                } else {
                    return Promise::reject(&JsValue::from_str("Unsupported file opened"));
                };

                let editor_state_lock = self.editor_state_lock.clone();
                let send_diagnostic = self.client_diagnostic_tx.clone();
                let send_notification = self.client_notification_tx.clone();

                return future_to_promise(async move {
                    let mut result = match editor_state_lock.try_write() {
                        Ok(mut editor_state) => {
                            process_notification(command, &mut editor_state, Some(&file_accessor))
                                .await
                        }
                        Err(_) => return Err(JsValue::from("unable to lock editor_state")),
                    };

                    let mut aggregated_diagnostics = vec![];
                    let mut notification = None;

                    if let Ok(ref mut response) = result {
                        aggregated_diagnostics.append(&mut response.aggregated_diagnostics);
                        notification = response.notification.take();
                    }

                    for (location, mut diags) in aggregated_diagnostics.into_iter() {
                        if let Ok(uri) = Url::parse(&location.to_string()) {
                            let value = PublishDiagnosticsParams {
                                uri,
                                diagnostics: clarity_diagnostics_to_lsp_type(&mut diags),
                                version: None,
                            };

                            let _ = send_diagnostic.call1(&JsValue::NULL, &encode_to_js(&value)?);
                        }
                    }

                    if let Some((level, message)) = notification {
                        let _ = send_notification.call2(
                            &JsValue::NULL,
                            &JsValue::from("window/showMessage"),
                            &encode_to_js(&ShowMessageParams {
                                message,
                                typ: level,
                            })?,
                        );
                    }

                    Ok(JsValue::TRUE)
                });
            }

            DidSaveTextDocument::METHOD => {
                let params: DidSaveTextDocumentParams = match decode_from_js(js_params) {
                    Ok(params) => params,
                    Err(err) => return Promise::reject(&JsValue::from(format!("error: {}", err))),
                };
                let uri = &params.text_document.uri;
                log!("> saved: {}", uri);

                let command = if let Some(contract_location) = get_contract_location(uri) {
                    LspNotification::ContractChanged(contract_location)
                } else if let Some(manifest_location) = get_manifest_location(uri) {
                    LspNotification::ManifestChanged(manifest_location)
                } else {
                    log!("Unsupported file opened");
                    return Promise::resolve(&JsValue::NULL);
                };

                let editor_state_lock = self.editor_state_lock.clone();
                let send_diagnostic = self.client_diagnostic_tx.clone();
                let send_notification = self.client_notification_tx.clone();

                return future_to_promise(async move {
                    let mut result = match editor_state_lock.try_write() {
                        Ok(mut editor_state) => {
                            process_notification(command, &mut editor_state, Some(&file_accessor))
                                .await
                        }
                        Err(_) => return Err(JsValue::from("unable to lock editor_state")),
                    };

                    let mut aggregated_diagnostics = vec![];
                    let mut notification = None;

                    if let Ok(ref mut response) = result {
                        aggregated_diagnostics.append(&mut response.aggregated_diagnostics);
                        notification = response.notification.take();
                    }

                    for (location, mut diags) in aggregated_diagnostics.into_iter() {
                        if let Ok(uri) = Url::parse(&location.to_string()) {
                            let value = PublishDiagnosticsParams {
                                uri,
                                diagnostics: clarity_diagnostics_to_lsp_type(&mut diags),
                                version: None,
                            };

                            let _ = send_diagnostic.call1(&JsValue::NULL, &encode_to_js(&value)?);
                        }
                    }

                    if let Some((level, message)) = notification {
                        let _ = send_notification.call2(
                            &JsValue::NULL,
                            &JsValue::from("window/showMessage"),
                            &encode_to_js(&ShowMessageParams {
                                message,
                                typ: level,
                            })?,
                        );
                    }
                    Ok(JsValue::TRUE)
                });
            }
            _ => {
                log!("unexpected notification ({})", method);
            }
        }

        return Promise::resolve(&JsValue::NULL);
    }

    #[wasm_bindgen(js_name=onRequest)]
    pub fn request_handler(&self, method: String, js_params: JsValue) -> Result<JsValue, JsValue> {
        log!("> request method: {}", method);

        match method.as_str() {
            Completion::METHOD => {
                let params: CompletionParams = decode_from_js(js_params)?;
                let file_url = params.text_document_position.text_document.uri;
                let location = get_contract_location(&file_url).ok_or(JsValue::NULL)?;
                let command = LspRequest::GetIntellisense(location);
                let editor_state = self
                    .editor_state_lock
                    .try_read()
                    .map_err(|_| JsValue::NULL)?;
                let lsp_response = process_request(command, &editor_state);

                return encode_to_js(&lsp_response.completion_items).map_err(|_| JsValue::NULL);
            }
            _ => {
                log!("unexpected request ({})", method);
            }
        }

        return Err(JsValue::NULL);
    }
}
