# Security

Please report a vulnerability privately rather than in a public issue: through
[a private advisory](https://github.com/sachesi/locus/security/advisories/new) on GitHub,
or by mail to sachesi <xsachesi@pm.me>. Say what you found, how to reproduce it and which
version you ran; a fix is worked out with you before anything is published.

Only the latest release gets fixes.

## What counts

Locus reads what every process on the machine makes public under `/proc`, and sends
signals to processes as the user running it. It asks for no privileges. A way to make it
signal a process other than the one the user chose, such as one given a pid that another
had when the list was read, or to show another user something the kernel would not show
them, is a vulnerability.

A process that Locus names or measures wrongly is a bug; please file it as an ordinary
issue.
