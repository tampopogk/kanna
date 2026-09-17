// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';
import MainTabBar from '../MainTabBar.vue';
import type { AgentTerminalAttempt } from '../../services/desktopServerClient';
vi.mock('vue-i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }));
afterEach(() => { document.body.innerHTML = ''; });
it('opens views from the plus menu with keyboard navigation and dismissal', async () => {
  const wrapper = mount(MainTabBar, { attachTo: document.body, props: { tabs: [{id:'agent',kind:'agent'}], activeTabId:'agent', newViews:[{id:'file',label:'Open file…'},{id:'shell',label:'Shell'}] } });
  await wrapper.get('.new-tab').trigger('click');
  await flushPromises();
  expect(document.activeElement?.textContent).toBe('Open file…');
  document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', {key:'ArrowDown',bubbles:true}));
  expect(document.activeElement?.textContent).toBe('Shell');
  (document.activeElement as HTMLButtonElement).click();
  expect(wrapper.emitted('new')).toEqual([['shell']]);
  await flushPromises();
  expect(document.querySelector('.new-tab-menu')).toBeNull();
  wrapper.unmount();
});
it('keeps content creation separate from the pane context menu, with keyboard access', async () => {
  const wrapper = mount(MainTabBar, { attachTo: document.body, props: {
    tabs: [{ id: 'agent', kind: 'agent' }], activeTabId: 'agent', paneId: 'pane-1',
    newViews: [{ id: 'file', label: 'Open file…', shortcut: '⌘P' }],
    paneActions: [{ id: 'split-horizontal', label: 'Split side by side' }],
  } });
  await wrapper.get('.new-tab').trigger('click');
  await flushPromises();
  expect(document.querySelector('.new-tab-menu')?.textContent).toBe('Open file…⌘P');
  expect(document.querySelector('.new-tab-menu kbd')?.textContent).toBe('⌘P');
  await wrapper.get('.main-tab-bar').trigger('contextmenu', { clientX: 20, clientY: 30 });
  await flushPromises();
  expect(document.querySelector('.new-tab-menu')).toBeNull();
  expect(document.activeElement?.textContent).toBe('Split side by side');
  (document.activeElement as HTMLButtonElement).click();
  expect(wrapper.emitted('layout')).toEqual([['split-horizontal']]);
  expect(wrapper.emitted('new')).toBeUndefined();
  await wrapper.get('.main-tab-bar').trigger('keydown', { key: 'F10', shiftKey: true });
  await flushPromises();
  document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await flushPromises();
  expect(document.querySelector('.pane-layout-menu')).toBeNull();
  expect(document.activeElement).toBe(wrapper.get('.main-tab-bar').element);
  wrapper.unmount();
});

it('closes only its pane through a separate control', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs: [{id:'agent',kind:'agent'}], activeTabId:'agent', canClosePane:true } });
  await wrapper.get('.close-pane').trigger('click');
  expect(wrapper.emitted('closePane')).toEqual([[]]);
  expect(wrapper.emitted('close')).toBeUndefined();
  await wrapper.setProps({canClosePane:false});
  expect(wrapper.find('.close-pane').exists()).toBe(false);
  wrapper.unmount();
});

it('closes only tabs visually to the right of the clicked tab, preserving Agent', async () => {
  const wrapper = mount(MainTabBar, { global:{mocks:{$t:(key:string)=>key}}, attachTo:document.body, props: {
    tabs:[{id:'file:left',kind:'file',filePath:'left'},{id:'diff',kind:'diff'},{id:'agent',kind:'agent'},{id:'file:right',kind:'file',filePath:'right'}],activeTabId:'file:left',
  } });
  await wrapper.get('[data-tab-id="diff"]').trigger('contextmenu');
  await flushPromises();
  (document.querySelector('.pane-layout-menu button') as HTMLButtonElement).click();
  expect(wrapper.emitted('close')).toEqual([['file:right']]);
  await wrapper.get('[data-tab-id="file:right"]').trigger('contextmenu');
  await flushPromises();
  expect((document.querySelector('.pane-layout-menu button') as HTMLButtonElement).disabled).toBe(true);
  document.activeElement?.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
  await flushPromises();
  expect(document.querySelector('.pane-layout-menu')).toBeNull();
  wrapper.unmount();
});

it('shows only an arrow and stage name, retaining detailed history choices', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs:[{id:'agent',kind:'agent'}],activeTabId:'agent',currentStage:'build',agentAttempts:[{id:'old',stage:'plan',startedAt:'yesterday',cwd:'/repo',archived:true,recordedLaunch:true,observedExitCode:0}] } });
  expect(wrapper.find('.main-tab-label').exists()).toBe(false);
  expect(wrapper.get('.stage-arrow svg').attributes('viewBox')).toBe('0 0 16 16');
  expect(wrapper.get('.stage-name').text()).toBe('build');
  expect(wrapper.get('select').text()).toContain('plan · attempt 1');
  const key = new KeyboardEvent('keydown',{key:']',metaKey:true,shiftKey:true,bubbles:true});
  let reachedTab = false;
  wrapper.get('.main-tab').element.addEventListener('keydown',()=>{reachedTab=true;});
  wrapper.get('select').element.dispatchEvent(key);
  expect(reachedTab).toBe(true);
  await wrapper.setProps({selectedAttempt:'old'});
  expect(wrapper.get('.stage-name').text()).toBe('plan');
  wrapper.unmount();
});

it('marks the append position, including empty panes, without retaining it on other targets', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs:[{id:'agent',kind:'agent'}],activeTabId:'agent',dropActive:true } });
  expect(wrapper.find('.drop-end').exists()).toBe(true);
  await wrapper.setProps({dropBefore:'agent'});
  expect(wrapper.find('.drop-end').exists()).toBe(false);
  expect(wrapper.get('[data-tab-id="agent"]').classes()).toContain('drop-before');
  await wrapper.setProps({tabs:[],dropBefore:undefined});
  expect(wrapper.find('.drop-end').exists()).toBe(true);
  await wrapper.setProps({dropActive:false});
  expect(wrapper.find('.drop-end').exists()).toBe(false);
  wrapper.unmount();
});

// Liveness is the owner daemon's session registry, not whether a final frame
// exists, so an ended attempt stays in history even when it is the newest one
// and its archive never arrived. Both the local and the remote task path feed
// this same list, so the option membership they produce is the same.
const attempt = (id: string, stage: string, over: Partial<AgentTerminalAttempt> = {}): AgentTerminalAttempt =>
  ({ id, stage, startedAt: id, cwd: '/repo', live: false, archived: false, recordedLaunch: true, observedExitCode: null, ...over });
const optionValues = (wrapper: ReturnType<typeof mount>) =>
  wrapper.findAll('option').map(option => option.attributes('value'));

it('keeps a finished attempt in history when it is newest, unarchived and exited', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs:[{id:'agent',kind:'agent'}],activeTabId:'agent',currentStage:'build',agentAttempts:[
    attempt('plan-run','plan',{ archived:true, observedExitCode:0 }),
    attempt('build-run','build',{ observedExitCode:0 }),
  ] } });
  expect(optionValues(wrapper)).toEqual(['', 'build-run', 'plan-run']);
  expect(wrapper.findAll('option')[1].text()).toBe('build · attempt 2 · build-run · history unavailable');
  expect(wrapper.findAll('option')[2].text()).toBe('plan · attempt 1 · plan-run');
  wrapper.unmount();
});

it('keeps the newest legacy attempt with no recorded launch in history', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs:[{id:'agent',kind:'agent'}],activeTabId:'agent',currentStage:'review',agentAttempts:[
    attempt('review-run','review',{ archived:true, observedExitCode:0 }),
    attempt('legacy-run','review',{ recordedLaunch:false }),
  ] } });
  expect(optionValues(wrapper)).toEqual(['', 'legacy-run', 'review-run']);
  expect(wrapper.findAll('option')[1].text()).toContain('· history unavailable');
  wrapper.unmount();
});

it('excludes only the live attempt, which Latest already shows', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs:[{id:'agent',kind:'agent'}],activeTabId:'agent',currentStage:'build',agentAttempts:[
    attempt('plan-run','plan',{ archived:true, observedExitCode:0 }),
    attempt('lost-run','plan',{ observedExitCode:1 }),
    attempt('live-run','build',{ live:true }),
  ] } });
  expect(optionValues(wrapper)).toEqual(['', 'lost-run', 'plan-run']);
  expect(wrapper.findAll('option')[0].text()).toBe('Latest · build');
  expect(wrapper.findAll('option')[1].text()).toBe('plan · attempt 2 · lost-run · history unavailable');
  expect(wrapper.findAll('option')[2].text()).toBe('plan · attempt 1 · plan-run');
  // Selecting history keeps the attempt's own identity and stage name.
  await wrapper.setProps({ selectedAttempt: 'lost-run' });
  expect(wrapper.get('select').element.value).toBe('lost-run');
  expect(wrapper.get('.stage-name').text()).toBe('plan');
  // The terminal exits: the registry stops naming it, so it joins history.
  await wrapper.setProps({ selectedAttempt: '', agentAttempts: [
    attempt('plan-run','plan',{ archived:true, observedExitCode:0 }),
    attempt('lost-run','plan',{ observedExitCode:1 }),
    attempt('live-run','build',{ archived:true, observedExitCode:0 }),
  ] });
  expect(optionValues(wrapper)).toEqual(['', 'live-run', 'lost-run', 'plan-run']);
  expect(wrapper.findAll('option')[1].text()).toBe('build · attempt 3 · live-run');
  wrapper.unmount();
});

it('leaves an all-live list with nothing but Latest', async () => {
  const wrapper = mount(MainTabBar, { props: { tabs:[{id:'agent',kind:'agent'}],activeTabId:'agent',currentStage:'build',agentAttempts:[
    attempt('live-run','build',{ live:true }),
  ] } });
  expect(optionValues(wrapper)).toEqual(['']);
  wrapper.unmount();
});
