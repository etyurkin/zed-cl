;;;; TCP localhost server for Master REPL (SBCL inet sockets; macOS, Linux, Windows).
;;;; Frames are 4-byte big-endian UTF-8 length followed by that many bytes of a printed sexp.

(eval-when (:compile-toplevel :load-toplevel :execute)
  (require :sb-bsd-sockets))

(defpackage :zed-cl.socket-server
  (:use :cl)
  (:import-from :zed-cl.config
                #:connection-file-path
                #:write-connection-file
                #:read-connection-file)
  (:export #:start-socket-server
           #:write-frame
           #:read-frame
           #:client-interrupt
           #:*running*))

(in-package :zed-cl.socket-server)

(defparameter *running* t
  "Server running flag")

(defparameter *bind-host* "127.0.0.1")

(defconstant +max-frame-octets+ (* 32 1024 1024))

(defvar *auth-token* nil
  "Shared secret every client must present before any other request.")

(define-condition client-interrupt (error) ()
  (:report "Evaluation interrupted")
  (:documentation "Signaled inside an evaluating thread to abort the eval."))

(defun getenv* (name)
  #+sbcl (sb-ext:posix-getenv name)
  #+ecl (ext:getenv name)
  #-(or sbcl ecl) (progn name nil))

(defun generate-auth-token ()
  "Prefer the token handed in by the spawning client (OS entropy); fall back
to the implementation RNG mixed with the clock."
  (or (getenv* "ZED_CL_AUTH_TOKEN")
      (let ((state (make-random-state t)))
        (string-downcase
         (format nil "~36,13,'0R~36,13,'0R~36,13,'0R"
                 (random (expt 36 13) state)
                 (random (expt 36 13) state)
                 (logxor (get-internal-real-time) (get-universal-time)))))))

(defun token-equal (a b)
  "Constant-time string comparison for the auth token."
  (and (stringp a)
       (stringp b)
       (= (length a) (length b))
       (zerop (reduce #'logior
                      (map 'list
                           (lambda (x y) (logxor (char-code x) (char-code y)))
                           a b)
                      :initial-value 0))))

(defun log-message (format-string &rest args)
  "Log a message to stderr"
  (apply #'format *error-output*
         (concatenate 'string "~&[Socket] " format-string "~%") args)
  (force-output *error-output*))

(defun host-vector ()
  #(127 0 0 1))

(defun delete-connection-file ()
  (let ((path (connection-file-path)))
    (when (probe-file path)
      (delete-file path))))

(defun connection-alive-p (host port)
  (declare (ignore host))
  (let ((test (make-instance 'sb-bsd-sockets:inet-socket
                             :type :stream :protocol :tcp)))
    (unwind-protect
         (handler-case
             (progn
               (sb-bsd-sockets:socket-connect test (host-vector) port)
               t)
           (error () nil))
      (ignore-errors (sb-bsd-sockets:socket-close test)))))

(defun cleanup-stale-server ()
  (let ((path (connection-file-path)))
    (when (probe-file path)
      (let ((conn (read-connection-file)))
        (when (and conn (connection-alive-p *bind-host* (cdr conn)))
          (log-message "Master REPL already running on port ~A" (cdr conn))
          (error "Master REPL already running - cannot start another instance"))
        (delete-connection-file)
        (log-message "Cleaned up stale connection file")))))

(defun create-server-socket ()
  (let ((socket (make-instance 'sb-bsd-sockets:inet-socket
                               :type :stream :protocol :tcp)))
    (setf (sb-bsd-sockets:sockopt-reuse-address socket) t)
    (sb-bsd-sockets:socket-bind socket (host-vector) 0)
    (sb-bsd-sockets:socket-listen socket 5)
    socket))

(defun local-port (socket)
  (nth-value 1 (sb-bsd-sockets:socket-name socket)))

(defun make-client-stream (socket)
  (sb-bsd-sockets:socket-make-stream
   socket
   :element-type '(unsigned-byte 8)
   :input t
   :output t
   :buffering :full))

(defun string-to-utf8 (string)
  #+sbcl (sb-ext:string-to-octets string :external-format :utf-8)
  #+ecl (ext:string-to-octets string :external-format :utf-8)
  #-(or sbcl ecl)
  (map '(simple-array (unsigned-byte 8) (*)) #'char-code string))

(defun utf8-to-string (octets)
  #+sbcl (sb-ext:octets-to-string octets :external-format :utf-8)
  #+ecl (ext:octets-to-string octets :external-format :utf-8)
  #-(or sbcl ecl)
  (map 'string #'code-char octets))

(defun write-u32-be (stream n)
  (write-byte (ldb (byte 8 24) n) stream)
  (write-byte (ldb (byte 8 16) n) stream)
  (write-byte (ldb (byte 8 8) n) stream)
  (write-byte (ldb (byte 8 0) n) stream))

(defun read-u32-be (stream)
  (let ((b0 (read-byte stream nil nil)))
    (unless b0
      (return-from read-u32-be nil))
    (let ((b1 (read-byte stream nil nil))
          (b2 (read-byte stream nil nil))
          (b3 (read-byte stream nil nil)))
      (unless (and b1 b2 b3)
        (return-from read-u32-be nil))
      (logior (ash b0 24) (ash b1 16) (ash b2 8) b3))))

(defun write-frame (stream message)
  (let* ((text (with-output-to-string (out)
                 (prin1 message out)))
         (octets (string-to-utf8 text))
         (len (length octets)))
    (when (> len +max-frame-octets+)
      (error "Frame too large: ~D bytes" len))
    (write-u32-be stream len)
    (write-sequence octets stream)
    (finish-output stream)
    t))

(defun read-frame (stream)
  (let ((len (read-u32-be stream)))
    (cond
      ((null len) :eof)
      ((> len +max-frame-octets+)
       (log-message "Frame too large: ~D" len)
       :eof)
      (t
       (let ((octets (make-array len :element-type '(unsigned-byte 8))))
         (let ((got (read-sequence octets stream)))
           (unless (= got len)
             (return-from read-frame :eof)))
         (handler-case
             ;; Never evaluate #. from the wire.
             (let ((*read-eval* nil))
               (read-from-string (utf8-to-string octets)))
           (error (e)
             (log-message "Error reading message: ~A" e)
             :eof)))))))

(defun authenticate-client (stream)
  "First frame must be (:type \"auth\" :token <secret>). Anything else is
rejected: the server evaluates arbitrary code, and localhost is reachable by
every local user."
  (let ((message (read-frame stream)))
    (cond
      ((eq message :eof) nil)
      ((and (listp message)
            (equal (getf message :type) "auth")
            (token-equal (getf message :token) *auth-token*))
       (write-frame stream (list :id (or (getf message :id) "handshake") :ok t))
       t)
      (t
       (log-message "Rejected unauthenticated client")
       (ignore-errors
         (write-frame stream
                      (list :id (or (and (listp message) (getf message :id)) "handshake")
                            :error "Authentication failed. Restart the master REPL and Zed.")))
       nil))))

(defun handle-client (client-socket message-handler)
  (let ((stream (make-client-stream client-socket)))
    (log-message "Client connected")
    (unwind-protect
         (when (authenticate-client stream)
           (loop while *running* do
             (handler-case
                 (let ((message (read-frame stream)))
                   (when (eq message :eof)
                     (log-message "Client disconnected")
                     (return))
                   (funcall message-handler message stream))
               ;; An interrupt that lands after the eval already finished hits
               ;; this thread outside eval; drop it instead of dying.
               (client-interrupt ()
                 (log-message "Late interrupt ignored")))))
      (ignore-errors (close stream))
      (ignore-errors (sb-bsd-sockets:socket-close client-socket)))))

(defun spawn-client-thread (client-socket message-handler)
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
  (loop while *running* do
    (handler-case
        (let ((client-socket (sb-bsd-sockets:socket-accept server-socket)))
          (spawn-client-thread client-socket message-handler))
      #+sbcl (sb-sys:interactive-interrupt ()
        (log-message "Interrupted")
        (setf *running* nil))
      (error (e)
        (log-message "Accept error: ~A" e)
        (sleep 0.1)))))

(defun start-socket-server (message-handler)
  "Start a TCP server on 127.0.0.1 with an ephemeral port.
   Writes ~/.zed-cl/repl-{impl}.json so clients can connect."
  (cleanup-stale-server)
  (setf *auth-token* (generate-auth-token))
  (let ((server-socket (create-server-socket)))
    (let ((port (local-port server-socket)))
      (write-connection-file *bind-host* port *auth-token*)
      (log-message "========================================")
      (log-message "Master REPL TCP Server")
      (log-message "Listening on ~A:~A" *bind-host* port)
      (log-message "========================================")
      (unwind-protect
           (accept-loop server-socket message-handler)
        (log-message "Shutting down socket server...")
        (ignore-errors (sb-bsd-sockets:socket-close server-socket))
        (delete-connection-file)
        (log-message "Socket server stopped")))))
