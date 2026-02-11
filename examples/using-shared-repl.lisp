;;;; Using Shared REPL - Example
;;;;
;;;; This file demonstrates how the shared master REPL works.
;;;; Functions defined in common-lisp-examples.lisp are available here!
;;;;
;;;; INSTRUCTIONS:
;;;; 1. Open common-lisp-examples.lisp in Zed
;;;; 2. Select and evaluate the functions there (Ctrl+Shift+Enter)
;;;; 3. Open THIS file in Zed
;;;; 4. Evaluate the examples below - they use functions from the other file!

;;; =============================================================================
;;; Using Functions from common-lisp-examples.lisp
;;; =============================================================================

;; Test greeting functions
;; These use the 'greet' function defined in common-lisp-examples.lisp
(greet "Alice")
(greet "Bob")

;; Test numeric functions
;; These use factorial and fibonacci from common-lisp-examples.lisp
(format t "~&Factorial of 5: ~A~%" (factorial 5))
(format t "~&Fibonacci of 10: ~A~%" (fibonacci 10))
(format t "~&Area of circle (radius 5): ~A~%" (calculate-area 5))

;; Test list operations
;; These use sum-list and filter-positive from common-lisp-examples.lisp
(format t "~&Sum of list: ~A~%" (sum-list '(1 2 3 4 5)))
(format t "~&Positive numbers: ~A~%" (filter-positive '(-3 -1 0 2 5 -8 10)))

;; Test string operations
;; These use shout and whisper from common-lisp-examples.lisp
(format t "~&Shouting: ~A~%" (shout "hello world"))
(format t "~&Whispering: ~A~%" (whisper "HELLO WORLD"))

;;; =============================================================================
;;; Combining Functions from Both Files
;;; =============================================================================

(defun greet-and-calculate (name n)
  "Combine greeting with factorial calculation"
  (declare (type string name)
           (type integer n))
  (format t "~&~A~%" (greet name))
  (format t "~&The factorial of ~A is ~A~%" n (factorial n)))

(defun process-names (names)
  "Process a list of names using greet-all and map-numbers"
  (declare (type list names))
  (format t "~&=== Greeting everyone ===~%")
  (greet-all names)
  (format t "~&~%=== Name lengths ===~%")
  (let ((lengths (map-numbers #'length names)))
    (format t "~&Lengths: ~A~%" lengths)
    (format t "~&Total length: ~A~%" (sum-list lengths))))

;;; =============================================================================
;;; Testing the Shared REPL
;;; =============================================================================

;; Try these to verify the shared REPL is working:

;; 1. Evaluate greet-and-calculate:
;; (greet-and-calculate "Charlie" 7)

;; 2. Evaluate process-names:
;; (process-names '("Alice" "Bob" "Charlie" "David"))

;; 3. Define a variable here and use it in the other file:
(defparameter *shared-message* "This variable is defined in using-shared-repl.lisp!"
  "A variable to test cross-file state")

;; 4. Now go to common-lisp-examples.lisp and try:
;; (format t "~&~A~%" *shared-message*)
;; It should work because both files share the same REPL!

;;; =============================================================================
;;; How This Works
;;; =============================================================================

;; The shared master REPL architecture means:
;;
;; 1. ONE master SBCL process runs at /tmp/zed-cl-master-repl.sock
;; 2. Each file gets its own Jupyter kernel (one per file in Zed)
;; 3. ALL kernels connect to the SAME master REPL
;; 4. Functions, variables, and state are shared across ALL files
;;
;; Benefits:
;; - Define helper functions in one file, use them everywhere
;; - Build libraries incrementally across multiple files
;; - True REPL-driven development experience
;; - No need to reload code when switching files

(format t "~&~%=== Shared REPL Demo Complete ===~%")
(format t "~&Functions from common-lisp-examples.lisp are available here!~%")
(format t "~&Try defining your own functions in either file and using them in both.~%")
