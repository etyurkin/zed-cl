;;;; Unix Domain Socket Server for Master REPL
;;;; Multi-implementation support (SBCL, ECL)

;; Load BSD sockets before defpackage (works on both SBCL and ECL)
(eval-when (:compile-toplevel :load-toplevel :execute)
  (require :sb-bsd-sockets))

(defpackage :zed-cl.socket-server
  (:use :cl)
  (:import-from :zed-cl.config
                #:get-profile-setting)
  (:export #:start-socket-server
           #:*socket-path*
           #:*running*))

(in-package :zed-cl.socket-server)

(defparameter *socket-path*
  (let ((lisp-impl (or (zed-cl.config:get-profile-setting :lisp--impl) "sbcl")))
    (format nil "/tmp/zed-cl-repl-~a.sock" lisp-impl))
  "Unix socket path for master REPL (implementation-specific to avoid conflicts)")

(defparameter *running* t
  "Server running flag")

(defun log-message (format-string &rest args)
  "Log a message to stderr"
  (apply #'format *error-output*
         (concatenate 'string "~&[Socket] " format-string "~%") args)
  (force-output *error-output*))

(defun cleanup-socket ()
  "Remove stale socket file if it exists and is not in use"
  (when (probe-file *socket-path*)
    ;; Try to connect to the socket - if it succeeds, another process is running
    (handler-case
        (let ((test-socket (make-instance 'sb-bsd-sockets:local-socket :type :stream)))
          (unwind-protect
               (progn
                 (sb-bsd-sockets:socket-connect test-socket *socket-path*)
                 ;; Connection succeeded - another instance is running
                 (log-message "Master REPL already running at ~A" *socket-path*)
                 (error "Master REPL already running - cannot start another instance"))
            (ignore-errors (sb-bsd-sockets:socket-close test-socket))))
      (error ()
        ;; Connection failed - socket is stale (no process listening)
        (delete-file *socket-path*)
        (log-message "Cleaned up stale socket file")))))

(defun create-server-socket ()
  "Create Unix domain socket server"
  (let ((socket (make-instance 'sb-bsd-sockets:local-socket
                               :type :stream)))
    (sb-bsd-sockets:socket-bind socket *socket-path*)
    (sb-bsd-sockets:socket-listen socket 5)
    socket))

(defun make-client-stream (socket)
  "Create buffered stream from client socket"
  (sb-bsd-sockets:socket-make-stream
   socket
   :element-type 'character
   :input t
   :output t
   :buffering :line))

(defun read-message (stream)
  "Read s-expression message from stream"
  (handler-case
      (read stream nil :eof)
    (end-of-file () :eof)
    (error (e)
      (log-message "Error reading message: ~A" e)
      :eof)))

(defun write-message (stream message)
  "Write s-expression message to stream"
  (handler-case
      (progn
        (prin1 message stream)
        (write-char #\Newline stream)
        (force-output stream)
        t)
    (error (e)
      (log-message "Error writing message: ~A" e)
      nil)))

(defun handle-client (client-socket message-handler)
  "Handle a client connection"
  (let ((stream (make-client-stream client-socket)))
    (log-message "Client connected")
    (unwind-protect
         (loop while *running* do
           (let ((message (read-message stream)))
             (when (eq message :eof)
               (log-message "Client disconnected")
               (return))
             ;; Dispatch to master-repl message handler
             ;; Handler should write response to same stream
             (funcall message-handler message stream)))
      (ignore-errors (close stream)))))

(defun spawn-client-thread (client-socket message-handler)
  "Spawn a thread to handle client connection (implementation-specific)"
  #+sbcl
  (sb-thread:make-thread
   (lambda () (handle-client client-socket message-handler))
   :name "socket-client")
  #+ecl
  (mp:process-run-function
   "socket-client"
   (lambda () (handle-client client-socket message-handler)))
  #-(or sbcl ecl)
  (handle-client client-socket message-handler))

(defun accept-loop (server-socket message-handler)
  "Accept client connections in a loop"
  (loop while *running* do
    (handler-case
        (let ((client-socket (sb-bsd-sockets:socket-accept server-socket)))
          ;; Handle each client in a separate thread
          (spawn-client-thread client-socket message-handler))
      #+sbcl (sb-sys:interactive-interrupt ()
        (log-message "Interrupted")
        (setf *running* nil))
      (error (e)
        (log-message "Accept error: ~A" e)
        (sleep 0.1)))))

(defun start-socket-server (message-handler)
  "Start Unix socket server
   MESSAGE-HANDLER: function (message stream) -> dispatches messages and writes response to stream"
  (cleanup-socket)
  (let ((server-socket (create-server-socket)))
    (log-message "========================================")
    (log-message "Master REPL Socket Server")
    (log-message "Listening on: ~A" *socket-path*)
    (log-message "========================================")
    (unwind-protect
         (accept-loop server-socket message-handler)
      ;; Cleanup
      (log-message "Shutting down socket server...")
      (ignore-errors (sb-bsd-sockets:socket-close server-socket))
      (cleanup-socket)
      (log-message "Socket server stopped"))))
