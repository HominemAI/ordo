# Console usability session: the script

For the owner, who runs the session. The method is
`plan-usability-testing.md` section 2: the moderator is silent, the tester
works alone, and the output is work items, not a report.

This script is for a less technical tester. Pick someone who has used a web
app and a password manager, and has never seen Ordo. They do not need to
know what Postgres or a container is.

## Before the session

Do these the day before. The tester never sees them.

1. Start the one image with `docker/compose.one.yml` on a clean volume.
2. Open its log and copy the setup code. Keep it ready on paper.
3. Make a second tenant name to use in task 4, for example `Practice`.
4. Have 1 small program file ready to deploy (the notes example).
5. Test the passkey on the tester's device. A laptop with a fingerprint
   reader or a phone is best.
6. Turn on screen and voice recording, with the tester's consent.
7. Print the **Lost passkey card** at the end of this page.

Time: 45 minutes for the tasks, 10 for the questions. Stop at 60.

## What you say first

Read this out. Do not add to it.

> We are testing the console, not you. If something is confusing, that is
> our mistake. Please think aloud. I will not answer questions during the
> tasks, but I will note them. You can stop at any time.

## The tasks

Give each task on a card. Read the card out once. Start a timer. Do not
help. A task stops when the tester says they are done, gives up, or hits 8
minutes.

### Task 1: Set up

Card: "This is a new console. Get yourself signed in, and keep yourself
safe from losing access."

- Entry state: the sign-in page is open. The setup code is on the paper.
  Hand over the paper only when the tester asks for "the code".
- Done when: a passkey exists, and the 10 recovery codes were shown and
  saved somewhere the tester names.

### Task 2: Link

Card: "Connect the console to the deployment it manages."

- Entry state: signed in, on the Health screen.
- Done when: the Deployment screen says linked.
- Have the claim code ready on a second paper. Hand it over only on request.

### Task 3: Deploy

Card: "Put this program on the deployment." Hand over the file.

- Entry state: linked.
- Done when: the Deploys screen shows the new version live.

### Task 4: Pause a tenant

Card: "A customer called `Practice` is misbehaving. Stop them from using
the app for now, without losing anything. Then let them back in."

- Entry state: tenant `Practice` exists.
- Done when: it shows paused, then active again.

### Task 5: Change a setting

Card: "The deployment should log people out sooner when they are idle. Make
the sign-in session shorter." Pick a real sign-in setting from the Sign-in
screen before the session and write it on the card.

- Entry state: linked.
- Done when: the screen shows the new value saved.

### Task 6: Recover from a lost passkey

Card: "Your laptop was stolen. You have a new one. Get back into the
console." Hand over the Lost passkey card, then take the old passkey away
(remove it in the browser, or use a different browser profile).

- Entry state: signed out, the old passkey unusable.
- Done when: the tester is signed in with a new passkey, and has a new set
  of recovery codes.

## What to watch

Write down each of these as it happens. Do not interrupt.

- Where the eyes go first on each screen.
- Where they stop for more than 10 seconds.
- Where they scroll, go back, or open the wrong screen.
- A word they read out loud and do not understand.
- The passkey box: do they know why it asked again?
- Whether they save the recovery codes, and where.
- What they do when a screen says "the deployment is not answering" or
  refuses.
- Any quote that shows confusion, surprise or relief. Write it word for
  word.

For each task, record:

- Done alone: yes or no.
- Time in seconds.
- Number of wrong turns.
- What help they asked for, and what you said.

## What counts as a failure

**Hard failure.** File a `severity:major` item.

- The tester cannot finish a task alone in 8 minutes.
- The tester finishes but thinks it failed, or the reverse.
- The tester makes a change they did not mean to (pauses the wrong tenant,
  unlinks).
- The tester loses access and does not know how to get it back.
- The tester does not save the recovery codes, or saves them next to the
  passkey.
- The tester asks for help twice on the same screen.

**Soft failure.** File a `severity:minor` item.

- More than 2 wrong turns on a task.
- A pause of more than 10 seconds on a screen with one obvious action.
- A word they ask about.
- A task takes more than twice the time you took doing it.

**Not a failure.**

- Slow typing, or a passkey that the browser itself fumbles.
- A question the tester answers themselves a moment later.
- Praise or complaints about looks that touch nothing they did.

## After the tasks

Ask each question once, on a scale of 1 (strongly disagree) to 5 (strongly
agree). Write the number and any reason they give.

1. "I knew what to do next on each screen."
2. "I knew if my change had worked."
3. "When something went wrong, the message told me how to fix it."
4. "I felt safe that I could not lock myself out."
5. "I understood why it asked me to touch my passkey again."
6. "I would be happy to run this for real."

Then 2 open questions:

- "What was the worst moment?"
- "What would you tell a friend who has to set this up tomorrow?"

## After the session

- File one item per distinct problem. The title says what went wrong. The
  body says the task, what they did, how long, and a quote.
- Label it `usability-testing`, and `severity:major` or `severity:minor`.
- Link each one to checklist 18o.
- Do not write a report.
- Let the fixes land before the next session. One week apart, at least.

## Lost passkey card

> Your laptop was stolen. You saved a list of 10 recovery codes when you
> first set up. They are in the place you chose then. The console is at the
> same address as before. Get back in.

If the tester did not save codes in task 1, they cannot do task 6 from the
codes. Then run the console's failsafe for them, from your own terminal,
and mark it a hard failure of task 1:

```
docker compose -f docker/compose.one.yml exec ordo ordo console recover
```
