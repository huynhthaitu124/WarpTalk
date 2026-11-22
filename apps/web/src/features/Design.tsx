import { useState, type ReactNode } from 'react';
import { House, Tray, Plus, GearSix, SquaresFour, Moon, Sun, MagnifyingGlass } from '@phosphor-icons/react';
import {
  Alert, Avatar, Badge, Brand, Button, Capsule, Card, CardBody, CardHeader, Checkbox, CommandMenuProvider, Dialog, DialogContent, DialogTrigger,
  Dropdown, DropdownContent, DropdownItem, DropdownLabel, DropdownSeparator, DropdownTrigger, EmptyState, Field, Input, Kbd, ListRow,
  Select, SidebarLink, SidebarNav, Skeleton, StatusPill, Switch, Table, TBody, TD, TH, THead, TR, Tabs, TabsContent, TabsList, TabsTrigger,
  Textarea, Tooltip, useCommandMenu, useToast, ROOM_STATUS_SAMPLE,
} from '@/ui';
import { useTheme } from '@/lib/theme';

const SURFACES = ['canvas', 'panel', 'surface-1', 'surface-2', 'surface-3', 'surface-4'];
const LINES = ['hairline', 'hairline-strong', 'hairline-tertiary', 'control-border'];
const INKS = ['ink', 'ink-muted', 'ink-subtle', 'ink-tertiary'];
const BRAND = ['primary', 'primary-hover', 'primary-pressed', 'link', 'focus'];
const SEMANTIC = ['success', 'warning', 'danger', 'info'];
const RADII = ['xs', 'sm', 'md', 'lg', 'xl', 'xxl', 'pill'];
const SPACING = [4, 8, 12, 16, 24, 32, 48];
const TYPE: [string, string, string][] = [
  ['display-xl', '80 / -3px', 'Display XL'],
  ['display-lg', '56 / -1.8px', 'Display LG'],
  ['display-md', '40 / -1px', 'Display MD'],
  ['headline', '28 / -0.6px', 'Headline'],
  ['card-title', '22 / -0.4px', 'Card title'],
  ['title', '18 / -0.2px', 'Title'],
  ['body', '14 / -0.05px', 'Body copy for the product UI'],
  ['body-sm', '13 / 0', 'Small body copy'],
  ['caption', '12 / 0', 'Caption and metadata'],
  ['eyebrow', '11 / +0.4px', 'EYEBROW LABEL'],
];

function Section({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section aria-labelledby={`h-${id}`} data-testid={`section-${id}`} className="flex flex-col gap-4">
      <h2 id={`h-${id}`} className="border-b border-hairline pb-2 text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">{title}</h2>
      {children}
    </section>
  );
}

function Swatch({ name }: { name: string }) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="h-12 rounded-md border border-hairline-strong" style={{ background: `var(--${name})` }} />
      <code className="text-caption text-ink-muted">--{name}</code>
    </div>
  );
}

function Demos() {
  const toast = useToast();
  const { setOpen } = useCommandMenu();
  const [sw, setSw] = useState(true);
  const [cb, setCb] = useState(true);
  const [sel, setSel] = useState('vi');
  return (
    <div className="flex flex-col gap-12">
      <Section id="colors" title="Colour tokens">
        <h3 className="text-body font-medium">Surface ladder</h3>
        <div className="grid grid-cols-3 gap-4 sm:grid-cols-6">{SURFACES.map((n) => <Swatch key={n} name={n} />)}</div>
        <h3 className="text-body font-medium">Hairlines</h3>
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">{LINES.map((n) => <Swatch key={n} name={n} />)}</div>
        <h3 className="text-body font-medium">Ink</h3>
        <div className="grid grid-cols-4 gap-4">{INKS.map((n) => <Swatch key={n} name={n} />)}</div>
        <h3 className="text-body font-medium">Primary (lavender)</h3>
        <div className="grid grid-cols-5 gap-4">{BRAND.map((n) => <Swatch key={n} name={n} />)}</div>
        <h3 className="text-body font-medium">Semantic</h3>
        <div className="grid grid-cols-4 gap-4">{SEMANTIC.map((n) => <Swatch key={n} name={n} />)}</div>
      </Section>

      <Section id="type" title="Type scale">
        <div className="flex flex-col gap-3">
          {TYPE.map(([k, meta, sample]) => (
            <div key={k} className="flex items-baseline gap-4 border-b border-hairline pb-3">
              <code className="w-28 shrink-0 text-caption text-ink-subtle">{k}</code>
              <span className="w-24 shrink-0 text-caption text-ink-tertiary">{meta}</span>
              <span className={`min-w-0 truncate font-display font-semibold text-${k}`}>{k.startsWith('display') ? sample.split(' ')[0] : sample}</span>
            </div>
          ))}
        </div>
      </Section>

      <Section id="radius" title="Radius & spacing">
        <div className="flex flex-wrap gap-4">
          {RADII.map((r) => (
            <div key={r} className="flex flex-col items-center gap-1.5">
              <div className="size-16 border border-hairline-tertiary bg-surface-2" style={{ borderRadius: `var(--radius-${r})` }} />
              <code className="text-caption text-ink-muted">{r}</code>
            </div>
          ))}
        </div>
        <div className="flex items-end gap-3">
          {SPACING.map((s) => (
            <div key={s} className="flex flex-col items-center gap-1.5">
              <div className="bg-primary" style={{ width: s, height: s }} />
              <code className="text-caption text-ink-muted">{s}</code>
            </div>
          ))}
        </div>
      </Section>

      <Section id="buttons" title="Buttons">
        <div className="flex flex-wrap items-center gap-3">
          <Button variant="primary">Primary</Button>
          <Button variant="secondary">Secondary</Button>
          <Button variant="ghost">Ghost</Button>
          <Button variant="danger">Danger</Button>
          <Button variant="danger-soft">Danger soft</Button>
          <Button variant="primary" loading>Loading</Button>
          <Button variant="secondary" disabled>Disabled</Button>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <Button variant="primary" size="sm">Small</Button>
          <Button variant="primary" size="md">Medium</Button>
          <Button variant="primary" size="lg">Large</Button>
          <Button variant="secondary" size="icon" aria-label="Add item"><Plus size={16} aria-hidden /></Button>
        </div>
      </Section>

      <Section id="forms" title="Form controls">
        <div className="grid max-w-2xl gap-4 sm:grid-cols-2">
          <Field label="Text input" hint="Helper text"><Input placeholder="Type here" /></Field>
          <Field label="Invalid input" error="This value is not valid"><Input defaultValue="oops" /></Field>
          <Field label="Select"><Select value={sel} onValueChange={setSel} options={[{ value: 'vi', label: 'Vietnamese' }, { value: 'en', label: 'English' }, { value: 'ja', label: 'Japanese' }]} /></Field>
          <Field label="Disabled"><Input disabled defaultValue="Read only" /></Field>
          <Field label="Textarea" className="sm:col-span-2"><Textarea placeholder="Longer text" /></Field>
          <Checkbox checked={cb} onCheckedChange={setCb} label="Checkbox" description="With a description" />
          <Switch checked={sw} onCheckedChange={setSw} label="Switch" />
        </div>
      </Section>

      <Section id="badges" title="Badges & status pills">
        <div className="flex flex-wrap gap-2">
          <Badge>Neutral</Badge><Badge tone="primary">Primary</Badge><Badge tone="success">Success</Badge><Badge tone="warning">Warning</Badge><Badge tone="danger">Danger</Badge><Badge tone="info">Info</Badge>
        </div>
        <div className="flex flex-wrap gap-2">{ROOM_STATUS_SAMPLE.map((s) => <StatusPill key={s} status={s} />)}</div>
        <div className="flex flex-wrap items-center gap-2">
          {['OWNER', 'ADMIN', 'MEMBER'].map((s) => <StatusPill key={s} status={s} />)}
          {['CONNECTED', 'INVITED', 'KICKED'].map((s) => <StatusPill key={s} status={s} />)}
          <Capsule><span>Info capsule</span></Capsule>
        </div>
      </Section>

      <Section id="cards" title="Cards, lists & tables">
        <div className="grid gap-4 md:grid-cols-2">
          <Card>
            <CardHeader title="Card title" description="Surface 1, hairline border, 12px radius" actions={<Button size="sm">Action</Button>} />
            <CardBody><p className="text-body text-ink-muted">Cards lift one step above the panel with a 1px hairline and no heavy shadow.</p></CardBody>
          </Card>
          <Card raised><CardBody><p className="text-body text-ink-muted">Raised card (surface-2, strong hairline).</p></CardBody></Card>
        </div>
        <div className="overflow-hidden rounded-lg border border-hairline bg-surface-1">
          <ListRow><span className="flex-1 text-body font-medium">Weekly sync</span><StatusPill status="IN_PROGRESS" /></ListRow>
          <ListRow><span className="flex-1 text-body font-medium">Customer call</span><StatusPill status="WAITING" /></ListRow>
        </div>
        <Table aria-label="Sample table">
          <THead><TR><TH>Name</TH><TH>Role</TH><TH>Status</TH></TR></THead>
          <TBody>
            <TR><TD>Ada Lovelace</TD><TD>Host</TD><TD><StatusPill status="CONNECTED" /></TD></TR>
            <TR><TD>Grace Hopper</TD><TD>Participant</TD><TD><StatusPill status="WAITING" /></TD></TR>
          </TBody>
        </Table>
      </Section>

      <Section id="overlays" title="Overlays">
        <div className="flex flex-wrap items-center gap-3">
          <Dialog>
            <DialogTrigger asChild><Button>Open dialog</Button></DialogTrigger>
            <DialogContent title="Dialog title" description="Dialogs trap focus and close with Escape." footer={<Button variant="primary">Confirm</Button>}>
              <p className="text-body text-ink-muted">Body content.</p>
            </DialogContent>
          </Dialog>
          <Dropdown>
            <DropdownTrigger asChild><Button>Open menu</Button></DropdownTrigger>
            <DropdownContent>
              <DropdownLabel>Actions</DropdownLabel>
              <DropdownItem icon={<Plus size={16} />} shortcut="C">New meeting</DropdownItem>
              <DropdownItem icon={<GearSix size={16} />}>Settings</DropdownItem>
              <DropdownSeparator />
              <DropdownItem danger>Delete</DropdownItem>
            </DropdownContent>
          </Dropdown>
          <Button onClick={() => setOpen(true)}><MagnifyingGlass size={16} aria-hidden />Command menu <Kbd>⌘K</Kbd></Button>
          <Tooltip content="Tooltip text"><Button variant="ghost">Hover me</Button></Tooltip>
          <Button onClick={() => toast({ tone: 'success', title: 'Saved', description: 'Toast message' })}>Show toast</Button>
        </div>
      </Section>

      <Section id="nav" title="Tabs & sidebar navigation">
        <Tabs defaultValue="a">
          <TabsList aria-label="Demo tabs"><TabsTrigger value="a">Overview</TabsTrigger><TabsTrigger value="b">Activity</TabsTrigger><TabsTrigger value="c">Settings</TabsTrigger></TabsList>
          <TabsContent value="a"><p className="text-body text-ink-muted">Overview tab content.</p></TabsContent>
          <TabsContent value="b"><p className="text-body text-ink-muted">Activity tab content.</p></TabsContent>
          <TabsContent value="c"><p className="text-body text-ink-muted">Settings tab content.</p></TabsContent>
        </Tabs>
        <div className="w-60 rounded-lg border border-hairline bg-canvas p-3">
            <SidebarNav label="Sidebar sample">
              <SidebarLink to="/design#rooms" active icon={<House weight="duotone" size={16} />}>Home</SidebarLink>
              <SidebarLink to="/design#meetings" icon={<SquaresFour weight="duotone" size={16} />}>Meetings</SidebarLink>
              <SidebarLink to="/design#settings" icon={<GearSix weight="duotone" size={16} />}>Settings</SidebarLink>
            </SidebarNav>
        </div>
      </Section>

      <Section id="feedback" title="Feedback & identity">
        <div className="grid gap-4 md:grid-cols-2">
          <div className="flex flex-col gap-3">
            <Alert tone="info" title="Info">Informational message.</Alert>
            <Alert tone="success" title="Success">It worked.</Alert>
            <Alert tone="warning" title="Warning">Be careful.</Alert>
            <Alert tone="danger" title="Error">Something failed.</Alert>
          </div>
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-2"><Skeleton className="h-4 w-2/3" /><Skeleton className="h-4 w-1/2" /><Skeleton className="h-10" /></div>
            <div className="flex items-center gap-2"><Avatar name="Ada Lovelace" size="sm" /><Avatar name="Grace Hopper" /><Avatar name="Linus T" size="lg" /><Kbd>⌘</Kbd><Kbd>K</Kbd></div>
          </div>
        </div>
        <EmptyState icon={<Tray size={20} weight="duotone" />} title="Nothing here yet" description="Empty states explain what belongs here and offer the next step." action={<Button variant="primary">Create something</Button>} />
      </Section>
    </div>
  );
}

export function DesignPage() {
  const { theme, toggle } = useTheme();
  return (
    <CommandMenuProvider items={[{ id: 'a', group: 'Navigate', label: 'Go to Meetings', run: () => undefined }, { id: 'b', group: 'Actions', label: 'Create meeting', run: () => undefined }]}>
      <div className="min-h-dvh bg-panel">
        <header className="sticky top-0 z-30 flex h-12 items-center justify-between border-b border-hairline bg-canvas px-6">
          <Brand />
          <span className="text-body text-ink-subtle">Design system</span>
          <Button variant="ghost" size="icon" aria-label="Toggle theme" onClick={toggle}>{theme === 'dark' ? <Sun size={16} aria-hidden /> : <Moon size={16} aria-hidden />}</Button>
        </header>
        <main id="main" tabIndex={-1} className="outline-none mx-auto max-w-5xl px-6 py-10 max-sm:px-4">
          <h1 className="mb-2 text-display-md font-semibold tracking-[-1px]">Design system</h1>
          <p className="mb-10 max-w-xl text-body-lg text-ink-subtle">Tokens and primitives used across WarpTalk. Dark by default, light as an equal.</p>
          <Demos />
        </main>
      </div>
    </CommandMenuProvider>
  );
}
