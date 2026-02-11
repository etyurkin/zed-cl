/// Jupyter kernel implementation using zeromq (pure Rust)

use anyhow::Result;
use aws_lc_rs::hmac;
use bytes::Bytes;
use common_rust::{MasterReplClient, ReplRequest};
use data_encoding::HEXLOWER;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use zeromq::{Socket, SocketRecv, SocketSend};

use crate::connection::ConnectionInfo;

/// Execution counter for kernel
struct ExecutionState {
    count: i32,
}

/// ZeroMQ delimiter
const DELIMITER: &[u8] = b"<IDS|MSG>";

/// Connection wrapper with HMAC signing
struct Connection<S> {
    socket: S,
    mac: Option<hmac::Key>,
}

impl<S> Connection<S> {
    fn new(socket: S, key: &str) -> Self {
        let mac = if key.is_empty() {
            None
        } else {
            Some(hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes()))
        };
        Connection { socket, mac }
    }
}

impl<S: SocketRecv> Connection<S> {
    async fn recv_message(&mut self) -> Result<(Vec<Bytes>, Value, Value, Value, Value)> {
        let multipart = self.socket.recv().await?;

        // Find delimiter
        let delimiter_index = multipart
            .iter()
            .position(|part| &part[..] == DELIMITER)
            .ok_or_else(|| anyhow::anyhow!("Missing delimiter"))?;

        let mut parts = multipart.into_vec();

        // Extract message parts after delimiter
        let jparts: Vec<_> = parts.drain(delimiter_index + 2..).collect();
        let expected_hmac = parts.pop().ok_or_else(|| anyhow::anyhow!("Missing HMAC"))?;
        parts.pop(); // Remove delimiter
        let identities = parts;

        // Verify HMAC if key is present
        if let Some(ref key) = self.mac {
            let sig = HEXLOWER.decode(&expected_hmac)?;
            let mut msg = Vec::new();
            // Only include header, parent_header, metadata, and content in the HMAC
            for part in &jparts[..4.min(jparts.len())] {
                msg.extend_from_slice(part);
            }
            hmac::verify(key, msg.as_ref(), sig.as_ref())
                .map_err(|_| anyhow::anyhow!("HMAC verification failed"))?;
        }

        // Parse JSON parts
        let header: Value = serde_json::from_slice(&jparts[0])?;
        let parent_header: Value = serde_json::from_slice(&jparts[1])?;
        let metadata: Value = serde_json::from_slice(&jparts[2])?;
        let content: Value = serde_json::from_slice(&jparts[3])?;

        Ok((identities, header, parent_header, metadata, content))
    }
}

impl<S: SocketSend> Connection<S> {
    async fn send_message(
        &mut self,
        identities: &[Bytes],
        msg_type: &str,
        parent_header: &Value,
        content: &Value,
    ) -> Result<()> {
        // Build header - extract session from parent_header, defaulting to empty string if missing
        let session = parent_header.get("session")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        debug!("send_message: msg_type={}, session={:?}, parent_header={}", msg_type, session, parent_header);

        let header = json!({
            "msg_id": uuid::Uuid::new_v4().to_string(),
            "msg_type": msg_type,
            "username": "kernel",
            "session": session,
            "date": chrono::Utc::now().to_rfc3339(),
            "version": "5.3"
        });

        let header_bytes = serde_json::to_vec(&header)?;
        let parent_header_bytes = serde_json::to_vec(parent_header)?;
        let metadata_bytes = b"{}".to_vec();
        let content_bytes = serde_json::to_vec(content)?;

        // Compute HMAC
        let hmac_str = if let Some(ref key) = self.mac {
            let mut ctx = hmac::Context::with_key(key);
            ctx.update(&header_bytes);
            ctx.update(&parent_header_bytes);
            ctx.update(&metadata_bytes);
            ctx.update(&content_bytes);
            let tag = ctx.sign();
            HEXLOWER.encode(tag.as_ref())
        } else {
            String::new()
        };

        // Build multipart message
        let mut parts: Vec<Bytes> = Vec::new();
        for id in identities {
            parts.push(id.clone());
        }
        parts.push(Bytes::from(DELIMITER.to_vec()));
        parts.push(Bytes::from(hmac_str.into_bytes()));
        parts.push(Bytes::from(header_bytes));
        parts.push(Bytes::from(parent_header_bytes));
        parts.push(Bytes::from(metadata_bytes));
        parts.push(Bytes::from(content_bytes));

        self.socket.send(zeromq::ZmqMessage::try_from(parts)
            .map_err(|e| anyhow::anyhow!("Failed to create ZMQ message: {:?}", e))?)
            .await?;
        Ok(())
    }
}

/// Jupyter kernel for Common Lisp
pub struct LispKernel {
    connection_info: ConnectionInfo,
    master_repl: Arc<RwLock<MasterReplClient>>,
    execution_state: Arc<RwLock<ExecutionState>>,
}

impl LispKernel {
    /// Create a new Lisp kernel
    pub async fn new(connection_file: PathBuf, master_repl_endpoint: String) -> Result<Self> {
        info!("Loading connection file...");
        let connection_info = ConnectionInfo::from_file(&connection_file)?;

        info!("Connecting to master REPL...");
        let mut master_repl = MasterReplClient::new(master_repl_endpoint);
        master_repl.connect()?;

        Ok(Self {
            connection_info,
            master_repl: Arc::new(RwLock::new(master_repl)),
            execution_state: Arc::new(RwLock::new(ExecutionState { count: 0 })),
        })
    }

    /// Run the kernel main loop
    pub async fn run(self) -> Result<()> {
        info!("Setting up ZeroMQ sockets...");

        // Create sockets
        let mut shell_socket = zeromq::RouterSocket::new();
        let mut iopub_socket = zeromq::PubSocket::new();
        let mut control_socket = zeromq::RouterSocket::new();

        // Subscribe to shutdown broadcasts from LSP server
        let mut shutdown_socket = zeromq::SubSocket::new();
        shutdown_socket.connect("tcp://127.0.0.1:5557").await?;
        shutdown_socket.subscribe("").await?;
        info!("Subscribed to shutdown broadcasts on tcp://127.0.0.1:5557");

        // Bind sockets
        shell_socket.bind(&self.connection_info.shell_address()).await?;
        iopub_socket.bind(&self.connection_info.iopub_address()).await?;
        control_socket.bind(&self.connection_info.control_address()).await?;

        info!("Kernel started on:");
        info!("  Shell: {}", self.connection_info.shell_address());
        info!("  IOPub: {}", self.connection_info.iopub_address());
        info!("  Control: {}", self.connection_info.control_address());

        // Create connections with HMAC
        let mut shell = Connection::new(shell_socket, &self.connection_info.key);
        let mut iopub = Connection::new(iopub_socket, &self.connection_info.key);
        let mut control = Connection::new(control_socket, &self.connection_info.key);

        // Spawn shutdown monitor task
        tokio::spawn(async move {
            info!("Starting shutdown monitor...");
            match shutdown_socket.recv().await {
                Ok(msg) => {
                    if let Some(first_part) = msg.into_vec().first() {
                        if first_part.as_ref() == b"SHUTDOWN" {
                            info!("Received shutdown broadcast from LSP - exiting");
                            std::process::exit(0);
                        }
                    }
                }
                Err(e) => {
                    error!("Shutdown monitor recv error: {}", e);
                    std::process::exit(1);
                }
            }
        });

        // Main message loop
        loop {
            tokio::select! {
                result = shell.recv_message() => {
                    match result {
                        Ok((identities, header, parent_header, metadata, content)) => {
                            if let Err(e) = self.handle_shell_message(&mut shell, &mut iopub, &identities, &header, &parent_header, &metadata, &content).await {
                                error!("Error handling shell message: {}", e);
                            }
                            // Send replies on shell socket
                            if let Err(e) = self.send_shell_reply(&mut shell, &identities, &header, &parent_header, &content).await {
                                error!("Error sending shell reply: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Error receiving shell message: {}", e);
                        }
                    }
                }
                result = control.recv_message() => {
                    match result {
                        Ok((identities, header, parent_header, _metadata, _content)) => {
                            if let Err(e) = self.handle_control_message(&mut control, &identities, &header, &parent_header).await {
                                error!("Error handling control message: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Error receiving control message: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Handle message on shell socket
    async fn handle_shell_message(
        &self,
        shell: &mut Connection<zeromq::RouterSocket>,
        iopub: &mut Connection<zeromq::PubSocket>,
        identities: &[Bytes],
        header: &Value,
        _parent_header: &Value,
        metadata: &Value,
        content: &Value,
    ) -> Result<()> {
        let msg_type = header["msg_type"].as_str().unwrap_or("");
        let msg_id = header["msg_id"].as_str().unwrap_or("");

        info!("Received message - type: {}, ID: {}", msg_type, msg_id);

        match msg_type {
            "execute_request" => {
                // Pass the incoming header as parent_header for our replies
                self.handle_execute_request(shell, iopub, identities, header, content, metadata)
                    .await?;
            }
            _ => {
                debug!("Message type {} will be handled in send_shell_reply", msg_type);
            }
        }

        Ok(())
    }

    /// Send reply on shell socket
    async fn send_shell_reply(
        &self,
        shell: &mut Connection<zeromq::RouterSocket>,
        identities: &[Bytes],
        header: &Value,
        _parent_header: &Value,
        _content: &Value,
    ) -> Result<()> {
        let msg_type = header["msg_type"].as_str().unwrap_or("");

        match msg_type {
            "kernel_info_request" => {
                info!("Sending kernel_info_reply");
                // Pass incoming header as parent_header for our reply
                self.send_kernel_info_reply(shell, identities, header).await?;
            }
            "complete_request" => {
                info!("Sending complete_reply");
                self.send_complete_reply(shell, identities, header).await?;
            }
            "shutdown_request" => {
                info!("Sending shutdown_reply");
                self.send_shutdown_reply(shell, identities, header).await?;
                std::process::exit(0);
            }
            "execute_request" => {
                // Already handled in handle_shell_message, reply sent from there
            }
            _ => {
                warn!("Unhandled message type: {}", msg_type);
            }
        }

        Ok(())
    }

    /// Handle control messages
    async fn handle_control_message(
        &self,
        control: &mut Connection<zeromq::RouterSocket>,
        identities: &[Bytes],
        header: &Value,
        _parent_header: &Value,
    ) -> Result<()> {
        let msg_type = header["msg_type"].as_str().unwrap_or("");

        match msg_type {
            "shutdown_request" => {
                info!("Handling shutdown_request on control socket");
                // Pass incoming header as parent_header for our reply
                self.send_shutdown_reply(control, identities, header).await?;
                std::process::exit(0);
            }
            _ => {
                debug!("Unhandled control message type: {}", msg_type);
            }
        }

        Ok(())
    }

    /// Send kernel info reply
    async fn send_kernel_info_reply(
        &self,
        socket: &mut Connection<zeromq::RouterSocket>,
        identities: &[Bytes],
        parent_header: &Value,
    ) -> Result<()> {
        let content = json!({
            "status": "ok",
            "protocol_version": "5.3",
            "implementation": "zed-cl-kernel",
            "implementation_version": "0.1.0",
            "language_info": {
                "name": "common-lisp",
                "version": "ANSI",
                "mimetype": "text/x-common-lisp",
                "file_extension": ".lisp",
            },
            "banner": "Common Lisp Kernel for Zed",
            "help_links": []
        });

        socket.send_message(identities, "kernel_info_reply", parent_header, &content).await
    }

    /// Handle execute request
    async fn handle_execute_request(
        &self,
        shell: &mut Connection<zeromq::RouterSocket>,
        iopub: &mut Connection<zeromq::PubSocket>,
        identities: &[Bytes],
        parent_header: &Value,
        content: &Value,
        metadata: &Value,
    ) -> Result<()> {
        let code = content["code"].as_str().unwrap_or("");
        let silent = content["silent"].as_bool().unwrap_or(false);

        // Extract file path from metadata
        let file_path = metadata["file_path"]
            .as_str()
            .or_else(|| metadata["cellId"].as_str())
            .or_else(|| metadata["source_file"].as_str())
            .map(|s| s.to_string());

        let file_line = metadata["line"]
            .as_u64()
            .or_else(|| metadata["start_line"].as_u64())
            .map(|l| l as u32);

        let file_character = metadata["character"]
            .as_u64()
            .or_else(|| metadata["start_character"].as_u64())
            .or_else(|| metadata["column"].as_u64())
            .map(|c| c as u32);

        info!("Execute request - silent: {}, code length: {}", silent, code.len());

        // Increment execution count
        let execution_count = {
            let mut state = self.execution_state.write().await;
            state.count += 1;
            state.count
        };

        // Send busy status
        if !silent {
            self.send_status(iopub, parent_header, "busy", identities).await?;
            self.send_execute_input(iopub, parent_header, code, execution_count, identities).await?;
        }

        // Forward to master REPL
        let request = ReplRequest::Eval {
            id: String::new(),
            code: code.to_string(),
            package: None,
            file_path,
            file_line,
            file_character,
        };

        let result = self.master_repl.write().await.send_request(request);

        // Send result back
        match result {
            Ok(response) => {
                use common_rust::ResponseData;

                match response.data {
                    ResponseData::EvalResult { output, values, error, traceback, displays } => {
                        if let Some(err) = error {
                            error!("Lisp evaluation error: {}", err);

                            if !silent && !output.is_empty() {
                                self.send_stream(iopub, parent_header, "stdout", &output, identities).await?;
                            }

                            let traceback_lines: Vec<String> = if let Some(tb) = traceback {
                                let lines: Vec<String> = tb.lines().map(|s| s.to_string()).collect();
                                if lines.len() > 1 {
                                    lines
                                } else {
                                    vec![]
                                }
                            } else {
                                vec![]
                            };

                            self.send_error_reply(shell, iopub, identities, parent_header, execution_count,
                                "LispError", err, traceback_lines).await?;
                        } else {
                            // Successful execution
                            if !silent {
                                if !output.is_empty() {
                                    self.send_stream(iopub, parent_header, "stdout", &output, identities).await?;
                                }

                                if !values.is_empty() || displays.is_some() {
                                    let result_text = if !values.is_empty() {
                                        values.join("\n")
                                    } else {
                                        String::new()
                                    };

                                    let mut data_map = serde_json::Map::new();

                                    if !result_text.is_empty() {
                                        data_map.insert("text/plain".to_string(), json!(result_text));
                                    }

                                    if let Some(ref displays_vec) = displays {
                                        let mut markdown_parts = Vec::new();

                                        for display_item in displays_vec {
                                            for (mime_type, content) in &display_item.data {
                                                if mime_type == "text/markdown" {
                                                    markdown_parts.push(content.clone());
                                                } else if mime_type == "application/json" {
                                                    match serde_json::from_str::<serde_json::Value>(content) {
                                                        Ok(json_value) => {
                                                            if json_value.is_object() {
                                                                data_map.insert(mime_type.clone(), json_value);
                                                            } else {
                                                                warn!("JSON value is not an object");
                                                                if !data_map.contains_key("text/plain") {
                                                                    data_map.insert("text/plain".to_string(), json!(content));
                                                                }
                                                            }
                                                        }
                                                        Err(e) => {
                                                            warn!("Failed to parse JSON: {}", e);
                                                            if !data_map.contains_key("text/plain") {
                                                                data_map.insert("text/plain".to_string(), json!(content));
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    data_map.insert(mime_type.clone(), json!(content));
                                                }
                                            }
                                        }

                                        if !markdown_parts.is_empty() {
                                            let markdown_combined = markdown_parts.join("\n\n---\n\n");
                                            data_map.insert("text/markdown".to_string(), json!(markdown_combined));
                                        }
                                    }

                                    if !data_map.is_empty() {
                                        let execute_result_content = json!({
                                            "execution_count": execution_count,
                                            "data": data_map,
                                            "metadata": {}
                                        });

                                        iopub.send_message(identities, "execute_result", parent_header, &execute_result_content).await?;
                                    }
                                }
                            }

                            let reply_content = json!({
                                "status": "ok",
                                "execution_count": execution_count
                            });

                            shell.send_message(identities, "execute_reply", parent_header, &reply_content).await?;
                        }
                    }
                    _ => {
                        error!("Unexpected response type");
                    }
                }
            }
            Err(e) => {
                error!("Master REPL error: {}", e);
                self.send_error_reply(shell, iopub, identities, parent_header, execution_count,
                    "RuntimeError", e.to_string(), vec![]).await?;
            }
        }

        if !silent {
            self.send_status(iopub, parent_header, "idle", identities).await?;
        }

        Ok(())
    }

    /// Send complete reply
    async fn send_complete_reply(
        &self,
        socket: &mut Connection<zeromq::RouterSocket>,
        identities: &[Bytes],
        parent_header: &Value,
    ) -> Result<()> {
        let content = json!({
            "status": "ok",
            "matches": [],
            "cursor_start": 0,
            "cursor_end": 0,
            "metadata": {}
        });

        socket.send_message(identities, "complete_reply", parent_header, &content).await
    }

    /// Send shutdown reply
    async fn send_shutdown_reply(
        &self,
        socket: &mut Connection<zeromq::RouterSocket>,
        identities: &[Bytes],
        parent_header: &Value,
    ) -> Result<()> {
        let content = json!({
            "status": "ok",
            "restart": false
        });

        socket.send_message(identities, "shutdown_reply", parent_header, &content).await
    }

    /// Send status message on IOPub
    async fn send_status(
        &self,
        socket: &mut Connection<zeromq::PubSocket>,
        parent_header: &Value,
        status: &str,
        identities: &[Bytes],
    ) -> Result<()> {
        let content = json!({ "execution_state": status });
        socket.send_message(identities, "status", parent_header, &content).await
    }

    /// Send execute_input message on IOPub
    async fn send_execute_input(
        &self,
        socket: &mut Connection<zeromq::PubSocket>,
        parent_header: &Value,
        code: &str,
        execution_count: i32,
        identities: &[Bytes],
    ) -> Result<()> {
        let content = json!({
            "code": code,
            "execution_count": execution_count
        });
        socket.send_message(identities, "execute_input", parent_header, &content).await
    }

    /// Send error reply
    async fn send_error_reply(
        &self,
        shell: &mut Connection<zeromq::RouterSocket>,
        iopub: &mut Connection<zeromq::PubSocket>,
        identities: &[Bytes],
        parent_header: &Value,
        execution_count: i32,
        ename: impl Into<String>,
        evalue: impl Into<String>,
        traceback: Vec<String>,
    ) -> Result<()> {
        let error_content = json!({
            "ename": ename.into(),
            "evalue": evalue.into(),
            "traceback": traceback
        });

        iopub.send_message(identities, "error", parent_header, &error_content).await?;

        let reply_content = json!({
            "status": "error",
            "execution_count": execution_count,
            "ename": error_content["ename"],
            "evalue": error_content["evalue"],
            "traceback": error_content["traceback"]
        });

        shell.send_message(identities, "execute_reply", parent_header, &reply_content).await
    }

    /// Send stream message on IOPub
    async fn send_stream(
        &self,
        socket: &mut Connection<zeromq::PubSocket>,
        parent_header: &Value,
        name: &str,
        text: &str,
        identities: &[Bytes],
    ) -> Result<()> {
        let content = json!({
            "name": name,
            "text": text
        });
        socket.send_message(identities, "stream", parent_header, &content).await
    }
}
