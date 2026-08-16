;;;; using-shared-repl.lisp
;;;;
;;;; Call functions defined in other files/packages. Same master REPL, same image.
;;;;
;;;; Eval first (cursor on each form, Ctrl+Shift+Enter):
;;;;   1. examples/common-lisp-examples.lisp  — CL-USER helpers (greet, factorial, …)
;;;;   2. examples/my-utils.lisp              — package MY-UTILS
;;;; Then eval the calls in this file.

(in-package :cl-user)

;;; =============================================================================
;;; From common-lisp-examples.lisp (CL-USER)
;;; =============================================================================

(greet "Alice")
(greet "Bob")

(format t "~&Factorial of 5: ~A~%" (factorial 5))
(format t "~&Fibonacci of 10: ~A~%" (fibonacci 10))
(format t "~&Area of circle (radius 5): ~A~%" (calculate-area 5))

(format t "~&Sum of list: ~A~%" (sum-list '(1 2 3 4 5)))
(format t "~&Positive numbers: ~A~%" (filter-positive '(-3 -1 0 2 5 -8 10)))

(format t "~&Shouting: ~A~%" (shout "hello world"))
(format t "~&Whispering: ~A~%" (whisper "HELLO WORLD"))

;;; =============================================================================
;;; From examples/my-utils.lisp (package MY-UTILS)
;;; =============================================================================

;; Exported symbols: package prefix my-utils:
(my-utils:add-numbers 5 10)
(my-utils:multiply-numbers 3 7)
(my-utils:format-greeting "World")

;; Non-exported: double colon
(my-utils::internal-helper 5)

;;; =============================================================================
;;; Combine both files from this package
;;; =============================================================================

(defun greet-and-calculate (name n)
  "CL-USER function that calls CL-USER greet/factorial from the other file."
  (declare (type string name)
           (type integer n))
  (format t "~&~A~%" (greet name))
  (format t "~&The factorial of ~A is ~A~%" n (factorial n)))

(defun process-names (names)
  "CL-USER function that calls greet-all, map-numbers, and sum-list."
  (declare (type list names))
  (format t "~&=== Greeting everyone ===~%")
  (greet-all names)
  (format t "~&~%=== Name lengths ===~%")
  (let ((lengths (map-numbers #'length names)))
    (format t "~&Lengths: ~A~%" lengths)
    (format t "~&Total length: ~A~%" (sum-list lengths))))

;; Uses MY-UTILS from this file's CL-USER package.
(defun greet-with-product (name a b)
  "Call MY-UTILS from CL-USER after eval'ing examples/my-utils.lisp."
  (format t "~&~A~%" (my-utils:format-greeting name))
  (format t "~&~A * ~A = ~A~%" a b (my-utils:multiply-numbers a b)))

;; (greet-and-calculate "Charlie" 7)
;; (process-names '("Alice" "Bob" "Charlie" "David"))
;; (greet-with-product "Nancy" 10 20)

(defparameter *shared-message* "This variable is defined in using-shared-repl.lisp!"
  "A variable to test cross-file state")

;; In common-lisp-examples.lisp you can eval:
;; (format t "~&~A~%" *shared-message*)
