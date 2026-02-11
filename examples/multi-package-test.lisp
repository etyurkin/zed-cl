;;;; multi-package-test.lisp - Test multi-package completion support
;;;;
;;;; This file demonstrates the shared REPL's multi-package support.
;;;; Evaluate sections in order to test package-qualified completion.

;;;; Step 1: Define a custom package for utilities
(defpackage :my-utils
  (:use :cl)
  (:export #:add-numbers
           #:multiply-numbers
           #:format-greeting))

(in-package :my-utils)

(declaim (ftype (function (integer integer) integer) add-numbers))
(defun add-numbers (a b)
  "Add two numbers together"
  (+ a b))

(defun multiply-numbers (a b)
  "Multiply two numbers"
  (* a b))

(defun format-greeting (name)
  "Format a greeting message"
  (format nil "Hello, ~A!" name))

;; This is an internal (non-exported) function
(defun internal-helper (x)
  "Internal helper - not exported"
  (* x 2))

;;;; Step 2: Define another package for data structures
(defpackage :my-data
  (:use :cl)
  (:export #:make-person
           #:person-name
           #:person-age))

(in-package :my-data)

(defstruct person
  "A simple person structure"
  name
  age)

;;;; Step 3: Define a main package that uses the others
(defpackage :my-app
  (:use :cl)
  (:import-from :my-utils #:add-numbers #:format-greeting)
  (:export #:run-app))

(in-package :my-app)

(defun run-app ()
  "Main application entry point"
  (let ((result (add-numbers 10 20))
        (greeting (format-greeting "World")))
    (format t "~A Result: ~A~%" greeting result)))

;;;; Step 4: Test completions
;;;;
;;;; Try these in the REPL to test multi-package completion:
;;;;
;;;; In package MY-APP:
;;;;   - Type (my-utils:  - should show add-numbers, multiply-numbers, format-greeting
;;;;   - Type (my-utils:: - should show add-numbers, multiply-numbers, format-greeting, internal-helper
;;;;   - Type (my-data:   - should show make-person, person-name, person-age
;;;;   - Type (add-       - should show add-numbers (imported into MY-APP)
;;;;
;;;; Switch to CL-USER:
(in-package :cl-user)
;;;;
;;;;   - Type (my-utils:  - should show exported symbols
;;;;   - Type (my-data:p  - should show person-related functions
;;;;
;;;; Test cross-package usage:

(defun test-multi-package ()
  "Test using symbols from multiple packages"
  ;; Use exported symbols with package prefix
  (let ((sum (my-utils:add-numbers 5 10))
        (product (my-utils:multiply-numbers 3 7))
        (person (my-data:make-person :name "Alice" :age 30)))
    (format t "Sum: ~A, Product: ~A~%" sum product)
    (format t "Person: ~A (age ~A)~%"
            (my-data:person-name person)
            (my-data:person-age person))))

;; Uncomment to test (should work):
;; (test-multi-package)

;; Uncomment to test internal symbol access (requires ::):
;; (my-utils::internal-helper 5)
