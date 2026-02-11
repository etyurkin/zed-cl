;;;; bootstrap.lisp - Common initialization for all zed-cl entry points
;;;;
;;;; Handles Quicklisp loading and ASDF configuration

(in-package :cl-user)

;;;; Load Quicklisp if available
(let ((quicklisp-init (merge-pathnames "quicklisp/setup.lisp" (user-homedir-pathname))))
  (when (probe-file quicklisp-init)
    (load quicklisp-init)))

;;;; Setup ASDF central registry
(defun setup-asdf-registry (script-path)
  "Add repl directory to ASDF central registry based on script path"
  (let* ((script-dir (make-pathname :name nil :type nil :defaults script-path)))
    ;; The script is in repl/, so add repl/ directly to registry
    (pushnew script-dir asdf:*central-registry* :test #'equal)))

;;;; Load system with ASDF (Quicklisp not needed for local systems)
(defun load-system-quiet (system-name)
  "Load ASDF system quietly"
  (let ((*standard-output* (make-broadcast-stream))
        (*error-output* (make-broadcast-stream)))
    (asdf:load-system system-name)))
