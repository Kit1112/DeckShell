/* An independently exec'd Xlib client. stdout acknowledgements come AFTER XSync.
 * stdin: NAME WINDOW MODE N VALUES...; WINDOW=0 selects root. MODE=-1 deletes.
 * "new" recreates a Steam window using the same connection; process restart is
 * separately driven by the observer. No CDP, controller or actual Steam involved.
 */
#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <xcb/xcb.h>

int main(void) {
    XInitThreads();
    Display *d = XOpenDisplay(NULL);
    if (!d) return 2;
    Window root = DefaultRootWindow(d);
    Window window = None;
    char line[1024], name[128];
    setvbuf(stdout, NULL, _IOLBF, 0);
    puts("ready");
    while (fgets(line, sizeof(line), stdin)) {
        /* Deliberate bypass of the Xlib interposer. This must remain a failing
         * absolute-barrier counterexample, even with LD_PRELOAD loaded. */
        if (!strcmp(line, "xcb\n")) {
            xcb_connection_t *c = xcb_connect(NULL, NULL);
            if (xcb_connection_has_error(c)) return 5;
            const char *atom = "GAMESCOPECTRL_BASELAYER_APPID";
            xcb_intern_atom_reply_t *r = xcb_intern_atom_reply(c,
                xcb_intern_atom(c, 0, strlen(atom), atom), NULL);
            if (!r) return 6;
            uint32_t queue[] = {413091, 769};
            xcb_window_t xr = xcb_setup_roots_iterator(xcb_get_setup(c)).data->root;
            xcb_change_property(c, XCB_PROP_MODE_REPLACE, xr, r->atom,
                                XCB_ATOM_CARDINAL, 32, 2, queue);
            free(r);
            free(xcb_get_input_focus_reply(c, xcb_get_input_focus(c), NULL));
            xcb_disconnect(c);
            puts("ok");
            continue;
        }
        if (!strcmp(line, "new\n")) {
            if (window) XDestroyWindow(d, window);
            window = XCreateSimpleWindow(d, root, 0, 0, 320, 200, 0, 0, 0);
            unsigned long id = 769;
            XChangeProperty(d, window, XInternAtom(d, "STEAM_GAME", False), XA_CARDINAL,
                            32, PropModeReplace, (unsigned char *)&id, 1);
            XMapWindow(d, window);
            XSync(d, False);
            printf("%lu\n", window);
            continue;
        }
        unsigned long target, values[16];
        int mode, n, used = 0;
        if (sscanf(line, "%127s %lu %d %d%n", name, &target, &mode, &n, &used) != 4 ||
            n < 0 || n > 16) return 3;
        char *p = line + used;
        for (int i = 0; i < n; i++) {
            char *end;
            values[i] = strtoul(p, &end, 0);
            if (end == p) return 4;
            p = end;
        }
        Atom a = XInternAtom(d, name, False);
        if (mode == -1) XDeleteProperty(d, target ? target : root, a);
        else XChangeProperty(d, target ? target : root, a, XA_CARDINAL, 32, mode,
                             (unsigned char *)values, n);
        XSync(d, False);
        puts("ok");
    }
    XCloseDisplay(d);
    return 0;
}
