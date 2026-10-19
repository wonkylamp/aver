import { createApp } from "./http/app.ts";
import type { OutboundMail } from "./mail/outbound.ts";

// Development entry point: mail is printed instead of sent.
const mailer = {
  async send(mail: OutboundMail): Promise<void> {
    console.log(`mail to ${mail.to} (reply-to ${mail.replyTo}): ${mail.subject}`);
  },
};

const port = Number(process.env.PORT ?? 8080);
createApp({ mailer }).server.listen(port, () => console.log(`relay-desk on :${port}`));
