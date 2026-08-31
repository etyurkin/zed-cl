;;;; config.lisp - Configuration parameters for zed-cl
;;;;
;;;; Centralized configuration for all zed-cl components

(defpackage :zed-cl.config
  (:use :cl)
  (:export
   #:*completion-package-whitelist*
   #:get-active-profile
   #:get-profile-setting
   #:data-dir
   #:connection-file-path
   #:write-connection-file
   #:read-connection-file
   #:read-connection-token))

(in-package :zed-cl.config)

;;;; Configuration File Path

(defparameter *config-path*
  (merge-pathnames ".zed-cl/config.json" (user-homedir-pathname))
  "Path to configuration file")

;;;; Profile-based Configuration

(defvar *cached-config* nil
  "Cached configuration to avoid re-reading file")

(defun read-config ()
  "Read and parse ~/.zed-cl/config.json"
  (when (and (probe-file *config-path*) (find-package :cl-json))
    (handler-case
        (with-open-file (stream *config-path* :direction :input)
          (funcall (find-symbol "DECODE-JSON" :cl-json) stream))
      (error (e)
        (format *error-output* "~&[Config] Error reading config: ~A~%" e)
        nil))))

(defun get-config ()
  "Get cached config or read it"
  (unless *cached-config*
    (setf *cached-config* (read-config)))
  *cached-config*)

(defun get-active-profile ()
  "Get the name of the active profile (default: \"sbcl\")"
  (let ((config (get-config)))
    (or (cdr (assoc :active--profile config))
        "sbcl")))

(defun get-default-profile ()
  "Get the default profile settings"
  (list (cons :lisp--impl "sbcl")
        (cons :system--index "system-index.db")
        (cons :completion--package--whitelist nil)))

(defun get-profile-setting (setting-name &optional default)
  "Get a setting from the active profile"
  (let* ((config (get-config))
         (active-profile-name (get-active-profile))
         (profiles (cdr (assoc :profiles config)))
         (profile-key (intern (string-upcase active-profile-name) :keyword))
         (active-profile (cdr (assoc profile-key profiles))))
    ;; If no profile found, use defaults
    (unless active-profile
      (setf active-profile (get-default-profile)))
    (or (cdr (assoc setting-name active-profile))
        default)))

(defun data-dir ()
  (merge-pathnames ".zed-cl/" (user-homedir-pathname)))

(defun connection-file-path (&optional impl)
  (merge-pathnames
   (format nil "repl-~a.json"
           (or impl (get-profile-setting :lisp--impl "sbcl")))
   (data-dir)))

(defun parse-connection-json (line)
  (when line
    (let ((host-pos (search "\"host\":" line))
          (port-pos (search "\"port\":" line)))
      (when (and host-pos port-pos)
        (let* ((host-start (+ host-pos 8))
               (host-end (position #\" line :start host-start))
               (host (when host-end (subseq line host-start host-end)))
               (port (parse-integer line :start (+ port-pos 7) :junk-allowed t)))
          (when (and host port)
            (cons host port)))))))

(defun harden-file-permissions (path)
  "Best-effort chmod 600: the file carries the auth token, and other users on
the machine must not be able to read it. On Windows the profile directory
ACLs already restrict other users."
  #+(and sbcl unix)
  (handler-case
      (progn
        (require :sb-posix)
        (let ((chmod (find-symbol "CHMOD" :sb-posix)))
          (when chmod
            (funcall chmod (namestring path) #o600))))
    (error () nil))
  #+(and ecl unix)
  (let ((chmod (or (find-symbol "CHMOD" :ext) (find-symbol "CHMOD" :si))))
    (when chmod
      (ignore-errors (funcall chmod (namestring path) #o600))))
  path)

(defun write-connection-file (host port &optional token)
  (let* ((path (connection-file-path))
         (tmp (pathname (concatenate 'string (namestring path) ".tmp"))))
    (ensure-directories-exist path)
    (with-open-file (out tmp :direction :output :if-exists :supersede
                         :if-does-not-exist :create)
      (if token
          (format out "{\"host\":~S,\"port\":~D,\"token\":~S}~%" host port token)
          (format out "{\"host\":~S,\"port\":~D}~%" host port)))
    (harden-file-permissions tmp)
    (when (probe-file path)
      (delete-file path))
    (rename-file tmp path)
    path))

(defun read-connection-token (&optional impl)
  "Auth token from the connection file, or NIL for a pre-auth server."
  (let ((path (connection-file-path impl)))
    (when (probe-file path)
      (with-open-file (in path)
        (let ((line (read-line in nil nil)))
          (when line
            (let ((pos (search "\"token\":" line)))
              (when pos
                (let* ((start (position #\" line :start (+ pos 8)))
                       (end (and start (position #\" line :start (1+ start)))))
                  (when end
                    (subseq line (1+ start) end)))))))))))

(defun read-connection-file (&optional impl)
  (let ((path (connection-file-path impl)))
    (when (probe-file path)
      (with-open-file (in path)
        (parse-connection-json (read-line in nil nil))))))

;;;; Completion Configuration

(defparameter *completion-package-whitelist* :unset
  "List of package names to include in completions.
   If :unset (default), includes all user-defined packages.
   If set to a list, ONLY includes packages explicitly listed (no magic defaults).")

;; Initialize whitelist from profile config
(let ((whitelist (get-profile-setting :completion--package--whitelist)))
  (when whitelist
    (setf *completion-package-whitelist* (mapcar #'string-upcase whitelist))))
