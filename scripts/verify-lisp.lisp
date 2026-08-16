;;;; verify-lisp.lisp - Load the master REPL and eval a trivial form.
;;;; Run from any cwd: sbcl --script scripts/verify-lisp.lisp

#+sbcl
(progn
  (setq *load-verbose* nil)
  (setq *compile-verbose* nil))

(defun exit-with-code (code)
  #+sbcl (sb-ext:exit :code code)
  #+ecl (ext:quit code)
  #-(or sbcl ecl) (uiop:quit code))

(defparameter *here*
  (make-pathname :name nil :type nil
                 :defaults (or *load-truename* *default-pathname-defaults*)))

(let ((quicklisp-init (merge-pathnames "quicklisp/setup.lisp"
                                       (user-homedir-pathname))))
  (when (probe-file quicklisp-init)
    (load quicklisp-init)))

(require :asdf)

(handler-case
    (let ((repl-dir (merge-pathnames "src/zed-cl-repl-impl/"
                                     (merge-pathnames "../" *here*))))
      (push (truename repl-dir) asdf:*central-registry*)
      (asdf:load-system :zed-cl/master-repl)
      (let ((eval-fn (intern "EVAL-FORMS-FROM-CODE" :zed-cl.master-repl)))
        (unless (equal (funcall eval-fn "(+ 1 2)") '(3))
          (error "eval-forms-from-code did not return (3)")))
      (exit-with-code 0))
  (error (e)
    (format *error-output* "✗ REPL Lisp verification failed: ~A~%" e)
    (exit-with-code 1)))
