;;;; TCP localhost server for Master REPL (SBCL inet sockets; macOS, Linux, Windows).

(eval-when (:compile-toplevel :load-toplevel :execute)
  (require :sb-bsd-sockets))

(defpackage :zed-cl.socket-server
  (:use :cl)
  (:import-from :zed-cl.config
                #:connection-file-path
                #:write-connection-file
                #:read-connection-file)
  (:export #:start-socket-server
           #:*running*))

(in-package :zed-cl.socket-server)

(defparameter *running* t
  "Server running flag")

(defparameter *bind-host* "127.0.0.1")

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
   :element-type 'character
   :input t
   :output t
   :buffering :line))

(defun read-message (stream)
  (handler-case
      (read stream nil :eof)
    (end-of-file () :eof)
    (error (e)
      (log-message "Error reading message: ~A" e)
      :eof)))

(defun handle-client (client-socket message-handler)
  (let ((stream (make-client-stream client-socket)))
    (log-message "Client connected")
    (unwind-protect
         (loop while *running* do
           (let ((message (read-message stream)))
             (when (eq message :eof)
               (log-message "Client disconnected")
               (return))
             (funcall message-handler message stream)))
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
  (let ((server-socket (create-server-socket)))
    (let ((port (local-port server-socket)))
      (write-connection-file *bind-host* port)
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
