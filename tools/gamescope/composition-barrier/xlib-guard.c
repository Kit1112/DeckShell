/* Experimental, process-local barrier for the Xlib writer identified in task 414.
 * NOT an X11 security boundary. Not installed or enabled by the session launcher.
 * Missing/malformed grant denies. Only an explicit CARDINAL/32 [1] allows.
 * The controller must initialise its layer before starting the guarded client.
 * Compile: cc -shared -fPIC -Wall -Wextra -Werror -o guard.so xlib-guard.c -lX11 -ldl -pthread
 */
#define _GNU_SOURCE
#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>

static int (*change_property)(Display *, Window, Atom, Atom, int, int,
                              const unsigned char *, int);
static int (*delete_property)(Display *, Window, Atom);
static pthread_once_t once = PTHREAD_ONCE_INIT;

static void resolve(void) {
    *(void **)(&change_property) = dlsym(RTLD_NEXT, "XChangeProperty");
    *(void **)(&delete_property) = dlsym(RTLD_NEXT, "XDeleteProperty");
    if (!change_property || !delete_property) abort();
}

/* Return 1 for protected root control, 2 for protected window flags. */
static int protected_property(Display *dpy, Window w, Atom property) {
    char *name = XGetAtomName(dpy, property);
    if (!name) return 0;
    int kind = 0;
    if (w == DefaultRootWindow(dpy) &&
        (!strcmp(name, "GAMESCOPECTRL_BASELAYER_APPID") ||
         !strcmp(name, "GAMESCOPECTRL_BASELAYER_WINDOW"))) kind = 1;
    if (!strcmp(name, "STEAM_OVERLAY") || !strcmp(name, "STEAM_INPUT_FOCUS") ||
        !strcmp(name, "_XWAYLAND_ALLOW_COMMITS")) kind = 2;
    XFree(name);
    return kind;
}

static int allowed(Display *dpy) {
    Atom grant = XInternAtom(dpy, "_DECKSHELL_EXPERIMENTAL_COMPOSITION_GRANT", False);
    Atom type = None;
    int format = 0;
    unsigned long count = 0, remaining = 0;
    unsigned char *data = NULL;
    int rc = XGetWindowProperty(dpy, DefaultRootWindow(dpy), grant, 0, 1, False,
                               XA_CARDINAL, &type, &format, &count, &remaining, &data);
    int ok = rc == Success && type == XA_CARDINAL && format == 32 && count == 1 &&
             remaining == 0 && data && *(unsigned long *)data == 1;
    if (data) XFree(data);
    return ok;
}

int XChangeProperty(Display *dpy, Window w, Atom property, Atom type, int format,
                    int mode, const unsigned char *data, int count) {
    pthread_once(&once, resolve);
    int kind = protected_property(dpy, w, property);
    if (!kind) return change_property(dpy, w, property, type, format, mode, data, count);
    /* Serialize the grant read and request with grant revocation on other clients.
     * Never wait for another client while the server is grabbed. Xlib round trips
     * below use this same connection. These are rare composition requests, not frames.
     * This does NOT revoke a focus already applied during a previous grant.
     */
    XLockDisplay(dpy);
    XGrabServer(dpy);
    int rc = 1;
    if (allowed(dpy)) {
        rc = change_property(dpy, w, property, type, format, mode, data, count);
    } else if (kind == 2) {
        unsigned long zero = 0;
        rc = change_property(dpy, w, property, XA_CARDINAL, 32, PropModeReplace,
                             (const unsigned char *)&zero, 1);
    }
    /* Root requests are omitted entirely; no forbidden intermediate value exists.
     * Window flags are normalised BEFORE publication, including malformed writes.
     */
    XUngrabServer(dpy);
    XFlush(dpy);
    XUnlockDisplay(dpy);
    return rc;
}

int XDeleteProperty(Display *dpy, Window w, Atom property) {
    pthread_once(&once, resolve);
    int kind = protected_property(dpy, w, property);
    if (!kind) return delete_property(dpy, w, property);
    XLockDisplay(dpy);
    XGrabServer(dpy);
    int rc = 1;
    if (allowed(dpy)) rc = delete_property(dpy, w, property);
    else if (kind == 2) {
        unsigned long zero = 0;
        rc = change_property(dpy, w, property, XA_CARDINAL, 32, PropModeReplace,
                             (const unsigned char *)&zero, 1);
    }
    XUngrabServer(dpy);
    XFlush(dpy);
    XUnlockDisplay(dpy);
    return rc;
}
