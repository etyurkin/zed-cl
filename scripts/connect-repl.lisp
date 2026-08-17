;;;; connect-repl.lisp - Interactive TCP client for the master REPL
;;;; Portable: sbcl --script scripts/connect-repl.lisp

#+ecl
(progn
  (setq c::*compile-verbose* nil)
  (setq c::*compile-print* nil)
  (setq c::*load-verbose* nil))
#+sbcl
(progn
  (setq *load-verbose* nil)
  (setq *compile-verbose* nil))

(let ((here (make-pathname :name nil :type nil
                           :defaults (or *load-truename* *default-pathname-defaults*))))
  (load (merge-pathnames "../src/zed-cl-repl-impl/config.lisp" here))
  (require :sb-bsd-sockets)
  (load (merge-pathnames "../src/zed-cl-repl-impl/socket-server.lisp" here)))

(defpackage :repl-client
  (:use :cl))

(in-package :repl-client)

(defvar *socket* nil)
(defvar *socket-stream* nil)
(defvar *request-counter* 0)

(defun exit-with-code (code)
  #+sbcl (sb-ext:exit :code code)
  #+ecl (ext:quit code)
  #-(or sbcl ecl) (uiop:quit code))

(defun host-vector ()
  #(127 0 0 1))

(defun connect-to-master-repl ()
  (let* ((lisp-impl (zed-cl.config:get-profile-setting :lisp--impl "sbcl"))
         (conn (zed-cl.config:read-connection-file lisp-impl)))
    (unless conn
      (format t "~&Error: Master REPL connection file not found at ~A~%"
              (zed-cl.config:connection-file-path lisp-impl))
      (format t "Open a .lisp file in Zed first so the LSP can start the REPL.~%")
      (exit-with-code 1))
    (handler-case
        (progn
          (setf *socket* (make-instance 'sb-bsd-sockets:inet-socket
                                        :type :stream :protocol :tcp))
          (sb-bsd-sockets:socket-connect *socket* (host-vector) (cdr conn))
          (setf *socket-stream* (sb-bsd-sockets:socket-make-stream
                                 *socket*
                                 :input t
                                 :output t
                                 :element-type '(unsigned-byte 8)))
          (format t "~&; Common Lisp REPL (~A)~%" lisp-impl)
          (format t "; Connected to ~A:~A~%" (car conn) (cdr conn))
          (format t "; Press Ctrl+D to exit~%~%")
          t)
      (error (e)
        (format t "~&Error connecting to master REPL: ~A~%" e)
        (exit-with-code 1)))))

(defun send-eval-request (code)
  (let ((id (format nil "repl-~D" (incf *request-counter*))))
    (zed-cl.socket-server:write-frame *socket-stream*
                                      `(:type "eval" :id ,id :code ,code :package nil))
    (let ((response (zed-cl.socket-server:read-frame *socket-stream*)))
      (when (eq response :eof)
        (error 'end-of-file :stream *socket-stream*))
      response)))

(defun repl-loop ()
  (loop
   (format t "~&CL> ")
   (force-output)
   (let ((input (read-line *standard-input* nil :eof)))
     (when (eq input :eof)
       (format t "~%")
       (return))
     (let ((trimmed-input (string-trim '(#\Space #\Tab #\Newline) input)))
       (unless (string= trimmed-input "")
         (when (or (string= trimmed-input "(quit)")
                   (string= trimmed-input "(exit)")
                   (string= trimmed-input ":q"))
           (format t "~%")
           (return))
         (handler-case
             (let ((response (send-eval-request trimmed-input)))
               (let ((output (getf response :output)))
                 (when (and output (not (string= output "")))
                   (format t "~A" output)))
               (let ((err (getf response :error)))
                 (if err
                     (format t "~&ERROR: ~A~%" err)
                     (dolist (value (getf response :values))
                       (format t "~&~A~%" value)))))
           (end-of-file ()
             (format t "~&Connection lost to master REPL~%")
             (return))
           (error (e)
             (format t "~&Error: ~A~%" e))))))))

(defun main ()
  (when (connect-to-master-repl)
    (unwind-protect
         (repl-loop)
      (when *socket-stream*
        (close *socket-stream* :abort t))
      (when *socket*
        (sb-bsd-sockets:socket-close *socket*)))))

(main)
