;;;; Common Lisp Examples
;;;;
;;;; Comprehensive examples demonstrating LSP features with type-aware completions.
;;;; Evaluate these in your Jupyter notebook to see intelligent completions in Zed!

;;; =============================================================================
;;; Basic Functions
;;; =============================================================================
(in-package :cl-user)
(mapcar #'(lambda (x) (* x x)) '(1 2 3))

(defun hello-world ()
  "Print a friendly greeting"
  (format t "Hello, World!~%"))

(hello-world)

(declaim (ftype (function (string) *) greet))
(defun greet (name)
  "Greet someone by name"
  (declare (type string name))
  (format t "Hello, ~a!~%" name))

(greet "Nancy")

(my-utils::multiply-numbers 10 20)
;;; =============================================================================
;;; Numeric Functions
;;; =============================================================================

(defun factorial (n)
  "Calculate factorial of n"
  (declare (type integer n))
  (if (<= n 1)
      1
      (* n (factorial (- n 1)))))

(defun fibonacci (n)
  "Calculate the nth Fibonacci number"
  (declare (type integer n))
  (if (<= n 1)
      n
      (+ (fibonacci (- n 1))
         (fibonacci (- n 2)))))

(defun calculate-area (radius)
  "Calculate area of a circle"
  (declare (type number radius))
  (let ((r-squared (* radius radius)))
    (* pi r-squared)))

(defun classify-number (n)
  "Classify a number as negative, zero, or positive"
  (declare (type number n))
  (cond
    ((< n 0) :negative)
    ((= n 0) :zero)
    ((> n 0) :positive)))

;;; =============================================================================
;;; List Operations
;;; =============================================================================

(defun sum-list (lst)
  "Sum all numbers in a list"
  (declare (type list lst))
  (if (null lst)
      0
      (+ (car lst) (sum-list (cdr lst)))))

(defun greet-all (names)
  "Greet multiple people by name"
  (declare (type list names))
  (dolist (name names)
    (format t "Hello, ~a!~%" name)))

(defun reverse-list (lst)
  "Reverse a list"
  (declare (type list lst))
  (if (null lst)
      nil
      (append (reverse-list (cdr lst)) (list (car lst)))))

(defun filter-positive (numbers)
  "Filter positive numbers from a list"
  (declare (type list numbers))
  (remove-if-not #'plusp numbers))

;;; =============================================================================
;;; String Operations
;;; =============================================================================

(defun shout (text)
  "Convert text to uppercase"
  (declare (type string text))
  (string-upcase text))

(defun whisper (text)
  "Convert text to lowercase"
  (declare (type string text))
  (string-downcase text))

(defun reverse-string (text)
  "Reverse a string"
  (declare (type string text))
  (reverse text))

;;; =============================================================================
;;; Higher-Order Functions
;;; =============================================================================

(defun map-numbers (fn lst)
  "Apply function to all numbers in list"
  (declare (type list lst))
  (mapcar fn lst))

(defun apply-twice (fn value)
  "Apply function twice to a value"
  (funcall fn (funcall fn value)))

;;; =============================================================================
;;; Variables (for testing completion)
;;; =============================================================================

(defvar *global-counter* 0
  "A global counter variable")

(defparameter *version* "1.0.0"
  "Version string")

(defconstant +pi+ 3.14159
  "Value of pi")

;;; =============================================================================
;;; Testing Instructions
;;; =============================================================================

;;; To test LSP completions:
;;;
;;; 1. Evaluate all functions above in your Jupyter notebook
;;; 2. In Zed, start typing function names:
;;;    - (gree     suggests: greet "${1:str}"$0
;;;    - (fact     suggests: factorial ${1:0}$0
;;;    - (sum-     suggests: sum-list ${1:'(1 2 3)}$0
;;;    - (calc     suggests: calculate-area ${1:1.0}$0
;;;
;;; 3. Notice how placeholders show example values based on type declarations!
;;;
;;; Type declarations drive intelligent completions:
;;;   - string  produces "${1:str}"     (with quotes)
;;;   - integer produces ${1:0}          (number)
;;;   - number  produces ${1:1.0}        (float)
;;;   - list    produces ${1:'(1 2 3)}   (example list)
