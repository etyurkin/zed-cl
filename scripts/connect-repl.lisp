;;;; connect-repl.lisp - Interactive REPL client for master REPL
;;;;
;;;; This provides a proper REPL experience that connects to the master REPL socket.
;;;; You can type normal Lisp expressions and see results, just like a regular REPL.

;; Suppress loading messages (must be done as early as possible)
#+ecl
(progn
  (setq c::*compile-verbose* nil)
  (setq c::*compile-print* nil)
  (setq c::*load-verbose* nil))
#+sbcl
(progn
  (setq *load-verbose* nil)
  (setq *compile-verbose* nil))

(let (#+sbcl (*load-verbose* nil)
      #+ecl (c::*load-verbose* nil))
  (require :sb-bsd-sockets))

;; Load Quicklisp if not already loaded
#-quicklisp
(let ((quicklisp-init (merge-pathnames "quicklisp/setup.lisp" (user-homedir-pathname))))
  (when (probe-file quicklisp-init)
    (load quicklisp-init :verbose nil :print nil)))

(require :asdf)
(unless (find-package :cl-json)
  (handler-bind ((style-warning #'muffle-warning))
    (asdf:load-system :cl-json :verbose nil)))

(defpackage :repl-client
  (:use :cl))

(in-package :repl-client)

(defvar *socket* nil)
(defvar *socket-stream* nil)
(defvar *request-counter* 0)

(defun read-config ()
  "Read Lisp implementation from active profile in config file"
  (let ((config-file (merge-pathnames ".zed-cl/config.json" (user-homedir-pathname))))
    (if (probe-file config-file)
        (with-open-file (stream config-file)
          (let* ((config (cl-json:decode-json stream))
                 (active-profile (or (cdr (assoc :active--profile config)) "sbcl"))
                 (profiles (cdr (assoc :profiles config)))
                 (profile (cdr (assoc (intern (string-upcase active-profile) :keyword) profiles))))
            (or (cdr (assoc :lisp--impl profile)) "sbcl")))
        "sbcl")))

(defun get-socket-path ()
  "Get the Unix socket path for the master REPL"
  (let ((impl (read-config)))
    (format nil "/tmp/zed-cl-repl-~a.sock" impl)))

(defun exit-with-code (code)
  "Exit with the given code (implementation-specific)"
  #+sbcl (sb-ext:exit :code code)
  #+ecl (ext:quit code))

(defun connect-to-master-repl ()
  "Connect to the master REPL socket"
  (let ((socket-path (get-socket-path))
        (lisp-impl (read-config)))
    (handler-case
        (progn
          (unless (probe-file socket-path)
            (format t "~&Error: Master REPL socket not found at ~A~%" socket-path)
            (format t "Make sure the LSP server is running (open a .lisp file in Zed)~%")
            (exit-with-code 1))

          (setf *socket* (make-instance 'sb-bsd-sockets:local-socket :type :stream))
          (sb-bsd-sockets:socket-connect *socket* socket-path)
          (setf *socket-stream* (sb-bsd-sockets:socket-make-stream
                                 *socket*
                                 :input t
                                 :output t
                                 :element-type 'character))

          (format t "~&; Common Lisp REPL (~A)~%" lisp-impl)
          (format t "; Connected to: ~A~%" socket-path)
          (format t "; Press Ctrl+D to exit~%")
          (format t "~%")
          t)
      (error (e)
        (format t "~&Error connecting to master REPL: ~A~%" e)
        (format t "Is the LSP server running?~%")
        (exit-with-code 1)))))

(defun send-eval-request (code)
  "Send an eval request to master REPL and return the response"
  (let ((id (format nil "repl-~D" (incf *request-counter*))))
    ;; Send request as s-expression
    (format *socket-stream* "~S~%"
            `(:type "eval" :id ,id :code ,code :package nil))
    (force-output *socket-stream*)

    ;; Read response
    (let ((response (read *socket-stream*)))
      response)))

(defun extract-values (response)
  "Extract values from response s-expression"
  (getf response :values))

(defun extract-error (response)
  "Extract error from response s-expression"
  (getf response :error))

(defun extract-output (response)
  "Extract output from response s-expression"
  (getf response :output))

(defun repl-loop ()
  "Main REPL loop"
  (loop
    (format t "~&CL> ")
    (force-output)

    (let ((input (read-line *standard-input* nil :eof)))
      (when (eq input :eof)
        (format t "~%")
        (return))

      (let ((trimmed-input (string-trim '(#\Space #\Tab #\Newline) input)))
        ;; Skip empty lines
        (unless (string= trimmed-input "")
          ;; Check for quit command
          (when (or (string= trimmed-input "(quit)")
                    (string= trimmed-input "(exit)")
                    (string= trimmed-input ":q"))
            (format t "~%")
            (return))

          ;; Send to master REPL
          (handler-case
              (let ((response (send-eval-request trimmed-input)))
                ;; Print any output
                (let ((output (extract-output response)))
                  (when (and output (not (string= output "")))
                    (format t "~A" output)))

                ;; Print error or values
                (let ((error (extract-error response)))
                  (if error
                      (format t "~&ERROR: ~A~%" error)
                      (let ((values (extract-values response)))
                        (dolist (value values)
                          (format t "~&~A~%" value))))))
            (end-of-file ()
              (format t "~&Connection lost to master REPL~%")
              (return))
            (error (e)
              (format t "~&Error: ~A~%" e))))))))

(defun main ()
  "Main entry point"
  (when (connect-to-master-repl)
    (unwind-protect
         (repl-loop)
      ;; Cleanup
      (when *socket-stream*
        (close *socket-stream* :abort t))
      (when *socket*
        (sb-bsd-sockets:socket-close *socket*)))))

;; Start the REPL
(main)
