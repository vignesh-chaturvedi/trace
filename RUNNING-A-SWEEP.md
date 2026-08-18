# Running a benchmark sweep — step by step

Someone asked you to run this because they don't have API credit and you do.

It runs a set of coding tasks against an AI model, records whether each one
was solved, and produces **one file** for you to send back.

- Nothing gets committed or pushed. You are not changing their project.
- Your API key never leaves your machine and is never written to any file.
- Total time: about **30–60 minutes**, mostly waiting on the model.

You do not need to understand the project. Follow the steps.

**On Windows, do Step 0 first.** On macOS or Linux, skip it.

---

## Step 0 — Windows only: set up WSL2

This project needs a Linux-style shell. Running it directly in PowerShell or
Command Prompt **will not work** — and it fails in an unhelpful way: the
harness can start a command but cannot stop one that overruns its time limit,
so a stuck task would hang the whole run instead of failing cleanly.

WSL2 gives you real Linux inside Windows and takes about ten minutes. Every
later step then works exactly as written.

**1.** Open **PowerShell as Administrator** (right-click → Run as
administrator) and run:

```
wsl --install
```

**2.** Reboot when it asks. After restarting, an Ubuntu window opens and sets
itself up. Choose a username and password when prompted — this is a Linux
account, separate from your Windows login.

> **You should see:** a prompt like `yourname@DESKTOP-ABC:~$`.
> If no window appeared, open **Ubuntu** from the Start menu.

**3.** Install **Docker Desktop for Windows** from <https://docker.com>. After
installing, open it and go to **Settings → Resources → WSL integration**, then
turn on the switch for **Ubuntu** and click **Apply & Restart**.

**4.** Check it from the Ubuntu window:

```bash
docker info
```

> **You should see:** a long block of text ending in server details.
> If you see "Cannot connect to the Docker daemon", the WSL integration switch
> in Step 0.3 is off, or Docker Desktop is not running.

**From here on, use the Ubuntu window for everything.** Not PowerShell, not
Command Prompt. Every command in this guide is typed there.

> **Important:** in Step 2 you will download the project. Do it in your Linux
> home folder — just run the commands as written, which puts it in `~/trace`.
> **Do not** put it under `/mnt/c/...`, that is, anywhere on your Windows
> drives. Docker cannot reliably reach files across that boundary and it is
> very slow. Step 5 will catch this if you get it wrong.

---

## Step 1 — Install two things

Windows users: you are in the Ubuntu window now, and Docker is already done.
You only need Rust below.

**Rust** (compiles the program):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then close and reopen your terminal. Check it worked:

```bash
cargo --version
```

> **You should see:** something like `cargo 1.89.0`.
> If you see "command not found", reopen your terminal, or run
> `source "$HOME/.cargo/env"`.

**Docker** (runs each task in an isolated container, so nothing touches your
real files). *Windows users: already done in Step 0 — skip to Step 2.*
Install Docker Desktop from <https://docker.com>, **open the app**, and wait
for its whale icon to stop animating. Check:

```bash
docker info
```

> **You should see:** a long block of text ending with server details.
> If you see "Cannot connect to the Docker daemon", Docker is installed but
> not running — open the Docker Desktop app and wait for it to start.

---

## Step 2 — Download the project

```bash
cd ~
git clone <REPO-URL-THEY-GAVE-YOU>
cd trace
```

> **Important:** do not edit anything in the `tasks/` folder. The results are
> only comparable if the tasks are exactly as shipped. The program records a
> fingerprint of them and will notice.

> **Windows:** the `cd ~` above matters. It puts the project in your Linux
> home folder, where Docker can reach it. Running this from `/mnt/c/...` looks
> like it works and then fails at Step 5.

---

## Step 3 — Put your API key in

Make your own private copy of the settings file:

```bash
cp .env.example .env
```

Open `.env` in any text editor. You will see lines like:

```
OPENAI_API_KEY=
GEMINI_API_KEY=
```

Paste your key after the `=` for **whichever provider you are using**, and
leave the other one empty. No quotes, no spaces:

```
GEMINI_API_KEY=AIza...your-actual-key...
```

Save the file.

> `.env` is permanently excluded from git. It cannot be committed by accident,
> not by you and not by them.

---

## Step 4 — Tell it which model to use

The project ships two settings files. Open the one for your provider:

- `trace.toml` — for OpenAI
- `gemini.toml` — for Google Gemini

Find the line near the top that says `name = "..."` and make sure it is the
exact model id you intend to pay for, for example:

```toml
name = "gpt-4.1-mini"
```

> **Check the price of that model before continuing.** On a cheap model the
> whole run costs well under a dollar. On a top-tier model, budget **$50–150**.
> The program cannot know your pricing — it reports whatever the settings file
> says.

If you are using Gemini, add `--config gemini.toml` to every command below.

---

## Step 5 — Run the check first (about 10 seconds, 2 API requests)

This verifies everything **before** spending your budget.

```bash
make preflight
```

> **You should see:**
>
> ```
> preflight
>
>   ok    api key                    GEMINI_API_KEY is set (53 chars)
>   ok    model id                   gemini-3.5-flash-lite
>   ok    layout lint                1 warning(s)
>   ok    tasks                      11 task(s), set 02e7038b06ff
>   ok    container runtime          docker responding
>   ok    workspace mount            visible at /workspace
>   ok    live api handshake         2 turns ok, tool calls work
>
> ready. `make contribute` will not waste your budget on a setup problem.
> ```
>
> **Every line must say `ok`.** If any says `FAIL`, it tells you what to fix.
> See Step 8. Do not continue until this is clean — that is the entire point
> of this step.

---

## Step 6 — Run the sweep (30–60 minutes)

```bash
make contribute
```

Leave it running. You will see a line appear for each task as it finishes:

```
sweep: 11 task(s) x 3 repeats on gemini-3.5-flash-lite
  pass  add-reverse                  r0   11 turns  $0.0000  51.5s
  pass  config-aliasing              r0   24 turns  $0.0000  92.1s
  fail  date-parser                  r0   30 turns  $0.0000  110.4s
  ...
```

`pass` and `fail` are both normal and both useful — the whole point is to find
out which tasks the model can solve. **`ERR` is different**: it means the
harness itself had a problem. A few are survivable; many means something is
wrong (usually your rate limit — see Step 8).

> **You will know it is finished when you see:**
>
> ```
> ==================================================================
>   DONE.
>   Send this one file back:  trace-results.md
>   Do not commit or push it.
> ==================================================================
> ```

If your terminal closes or you stop it partway, just run `make contribute`
again. Nothing is damaged; it starts a fresh run.

---

## Step 7 — Send the file back

The file `trace-results.md` is now in the project folder. Send it however you
like — email, WhatsApp, anything. It is small (usually under 100 KB).

> **Windows:** the file is inside WSL, so Windows Explorer will not show it in
> the usual places. From the Ubuntu window, run `explorer.exe .` to open the
> project folder in Explorer, then drag the file out. Or copy it across with
> `cp trace-results.md /mnt/c/Users/<your-windows-name>/Desktop/`.

**Please do not commit or push it.** It is their result to record, in their
repository.

### What is in the file, and what is not

It contains pass/fail for each task, how many steps the model took, token
counts, timings, cost, and trimmed output from the tasks that failed.

**It does not contain your API key.** Four separate things ensure this:

1. The programs that run each task are started with your key removed from
   their environment entirely — they cannot read it.
2. Every line written to any log is scanned and stripped of known secrets as
   it is written, not when it is displayed.
3. Before this file is created it is scanned for anything key-shaped. If
   anything is found, **the file is not written at all**.
4. When that scan reports a problem, it never prints the value it found.

The file is plain text. **Open it and read the whole thing before sending.**
You are the one taking the risk, so you should be able to see exactly what you
are sharing.

---

## Step 8 — If something goes wrong

**`No .env found`**
You skipped Step 3. Run `cp .env.example .env` and paste your key.

**`FAIL  api key — ... is not set`**
The variable name in `.env` does not match the one in the settings file. They
must be spelled identically. If your key is in `GEMINI_API_KEY`, run commands
with `--config gemini.toml`.

**`FAIL  model id — "PUT-THE-EXACT-MODEL-ID-HERE" is a placeholder`**
Step 4. Put a real model id in the settings file.

**`FAIL  container runtime — docker is installed but not responding`**
Open the Docker Desktop app and wait for it to finish starting.

**`FAIL  workspace mount — not visible inside the container`**
Docker cannot see the folder you are working in. Move the project inside your
home folder (for example `~/trace`) and try again, or add the folder under
Docker Desktop → Settings → Resources → File sharing.

**`FAIL  live api handshake — HTTP 401`**
The key is wrong, expired, or belongs to a different provider than the URL in
the settings file.

**`FAIL  live api handshake — HTTP 404`**
The model id does not exist on your account. Check the exact spelling in your
provider's documentation.

**`HTTP 429` / "quota exceeded", or many `ERR` lines**
Your plan allows fewer requests than the settings file assumes. Open the
settings file, find `requests_per_minute`, and lower it (try `5`). If it is a
daily cap, run a smaller batch instead and say so when you send the file:

```bash
cargo run --release --bin trace -- bench run --repeats 3 --limit 3 --container --bundle trace-results.md
```

**A partial result is genuinely useful.** Send it, and mention which tasks ran.

**Anything else**
Send the error text along with whatever `trace-results.md` was produced. The
error is often more informative than the results.

### Windows-specific

**`'make' is not recognized`, `bash: command not found`, `'cargo' is not
recognized`**
You are in PowerShell or Command Prompt. Open the **Ubuntu** window from the
Start menu and run everything there. See Step 0.

**`FAIL  workspace mount — not visible inside the container`, and the project
is under `/mnt/c/...`**
The project is on a Windows drive, which Docker cannot reliably reach from
WSL. Move it into the Linux side and try again:

```bash
cd ~ && git clone <REPO-URL> && cd trace
```

**Everything is very slow, or the build takes many minutes**
Same cause: the project is on `/mnt/c/...`. File access across that boundary
is slow enough to matter. Move it to `~` as above.

**`docker info` works in PowerShell but not in Ubuntu**
Docker Desktop → Settings → Resources → **WSL integration** → enable your
Ubuntu distro → Apply & Restart.

**Windows without WSL**
Not supported, and it fails in a way that wastes your time rather than telling
you: the harness can start a command but cannot stop one that overruns, so a
stuck task hangs the run instead of failing. Use WSL2 — Step 0.
