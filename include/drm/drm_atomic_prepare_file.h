/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_FILE_H__
#define __DRM_ATOMIC_PREPARE_FILE_H__

struct drm_prepare_ticket;
struct file;

/*
 * Create an observation file for a provider-authorized preparation ticket. Failure
 * leaves the borrowed ticket unchanged. The owned file reference retains the
 * ticket; final fput cancels it, including for an unpublished file. get_file/dup
 * shares that file lifetime. Kernel ticket references do not prevent cancellation.
 * A closed descriptor need not be the final reference while duplicates or active
 * operations retain the file. Prompt revocation uses explicit ticket cancellation.
 *
 * Pending tickets have no poll events, ready tickets report readable, consumed
 * tickets report hangup, and cancellation/source failure report error and hangup.
 * Readability is readiness observation, not a byte stream or successful reserve.
 * DRM_IOCTL_PREPARE_QUERY returns the native ticket status as a UAPI value.
 * No pixel, modesetting or descriptor-installation API is exposed.
 * The eventual issuer must reserve descriptors with O_CLOEXEC and finish fallible
 * setup before publication. Issuing a file does not validate the ticket's scope.
 */
struct file *drm_prepare_ticket_file_create(struct drm_prepare_ticket *ticket);

/*
 * Return an owned kernel ticket reference from a borrowed live file, or -EINVAL
 * for another file type. Lookup neither reserves nor validates source authority.
 * Retaining the result does not prevent cancellation when the file is closed.
 */
struct drm_prepare_ticket *drm_prepare_ticket_file_get_ticket(struct file *file);

#endif
