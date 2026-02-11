;;;; start-master-repl.lisp - Entry point for master REPL server
;;;;
;;;; This starts the singleton Lisp process that handles all code evaluation
;;;; Multi-implementation support: SBCL, ECL

(in-package :cl-user)

;; Load required modules (portable)
(require :asdf)
(require :sb-bsd-sockets)

;; Load bootstrap utilities (ASDF setup)
(load (merge-pathnames "bootstrap.lisp"
                       (make-pathname :name nil :type nil
                                     :defaults (or *load-truename* *compile-file-truename*))))

;; Setup ASDF to find our systems
(setup-asdf-registry (or *load-truename* *compile-file-truename*))

;; Load the minimal master REPL system (no longer needs full Jupyter stack)
(handler-case
    (progn
      (load-system-quiet :zed-cl/master-repl)
      (format *error-output* "~&[Master REPL] System loaded successfully~%")
      (force-output *error-output*))
  (error (e)
    (format *error-output* "~&[Master REPL] FATAL: Failed to load zed-cl/master-repl system: ~A~%" e)
    (force-output *error-output*)
    #+sbcl (sb-ext:exit :code 1)
    #+ecl (ext:quit 1)
    #-(or sbcl ecl) (uiop:quit 1)))

;; Start the master REPL server
;; This function blocks forever, handling client connections
(zed-cl.master-repl:start-master-repl)
