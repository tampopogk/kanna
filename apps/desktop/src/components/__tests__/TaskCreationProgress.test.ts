import { mount, flushPromises } from '@vue/test-utils';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TaskCreationProgress from '../TaskCreationProgress.vue';
vi.mock('../ReadOnlyTerminal.vue', () => ({ default: { props: ['output', 'identity'], template: '<pre>{{ output }}</pre>' } }));
const read = vi.hoisted(() => vi.fn());
vi.mock('../../services/desktopServerClient', () => ({ readTaskCreationProgress: read }));
afterEach(() => { vi.useRealTimers(); read.mockReset(); });
describe('live task creation output', () => {
  it('shows actual phases and incremental output, retaining it after agent startup', async () => {
    vi.useFakeTimers();
    read.mockResolvedValueOnce({ phase: 'Git fetch origin', status: 'running', output: 'Git fetch origin' })
      .mockResolvedValueOnce({ phase: 'Creating workspace / git worktree', status: 'running', output: 'Creating workspace / git worktree\nfetch diagnostic' })
      .mockResolvedValueOnce({ phase: 'Running workspace setup', status: 'running', output: 'first line' })
      .mockResolvedValueOnce({ phase: 'Running workspace setup', status: 'running', output: 'first line\nsecond line' })
      .mockResolvedValueOnce({ phase: 'Task created', status: 'succeeded', output: 'first line\nsecond line' });
    const wrapper = mount(TaskCreationProgress, { attachTo: document.body, props: { taskId: 'abcd1234', creating: true } });
    await flushPromises();
    expect(wrapper.text()).toContain('Git fetch origin');
    await vi.advanceTimersByTimeAsync(250);
    expect(wrapper.text()).toContain('Creating workspace');
    await vi.advanceTimersByTimeAsync(250);
    expect(wrapper.text()).toContain('first line');
    expect(wrapper.text()).not.toContain('second line');
    await vi.advanceTimersByTimeAsync(250);
    expect(wrapper.text()).toContain('second line');
    await vi.advanceTimersByTimeAsync(250);
    await wrapper.setProps({ creating: false });
    expect(wrapper.get('pre').text()).toContain('second line');
    expect(wrapper.isVisible()).toBe(false);
    expect(wrapper.find('summary').exists()).toBe(false);
    expect(wrapper.emitted('completed')).toContainEqual(['succeeded']);
    await wrapper.setProps({ selected: true });
    expect(wrapper.isVisible()).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(read).toHaveBeenCalledTimes(5);
    wrapper.unmount();
  });
  it('keeps a failed setup diagnostic visible and stops polling', async () => {
    vi.useFakeTimers();
    read.mockResolvedValue({ phase: 'Task creation failed', status: 'failed', output: 'stderr: CONTROLLED_FAILURE', error: 'workspace setup failed with exit status: 23' });
    const wrapper = mount(TaskCreationProgress, { attachTo: document.body, props: { taskId: 'abcd1234', creating: true } });
    await flushPromises();
    expect(wrapper.text()).toContain('CONTROLLED_FAILURE');
    expect(wrapper.get('pre').text()).toContain('23');
    expect(wrapper.emitted('completed')).toContainEqual(['failed']);
    await vi.advanceTimersByTimeAsync(1000);
    expect(read).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
  it('reads final stderr when the create response beats the last poll', async () => {
    read.mockResolvedValueOnce({ phase: 'Running workspace setup', status: 'running', output: 'FIRST' })
      .mockResolvedValueOnce({ phase: 'Task creation failed', status: 'failed', output: 'FIRST\nFINAL_STDERR' });
    const wrapper = mount(TaskCreationProgress, { attachTo: document.body, props: { taskId: 'abcd1234', creating: true } });
    await flushPromises();
    await wrapper.setProps({ error: '\u001b[31mexit 23\u001b[0m' });
    await flushPromises();
    expect(wrapper.get('pre').text()).toContain('FINAL_STDERR');
    expect(wrapper.get('pre').text()).toContain('\u001b[31mexit 23\u001b[0m');
    wrapper.unmount();
  });
  it('discards responses from a previous task and cancels polling on unmount', async () => {
    vi.useFakeTimers();
    let resolveOld!: (value: unknown) => void;
    read.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve; }))
      .mockResolvedValue({ phase: 'Running workspace setup', status: 'running', output: 'new task' });
    const wrapper = mount(TaskCreationProgress, { attachTo: document.body, props: { taskId: 'old', creating: true } });
    await wrapper.setProps({ taskId: 'new' });
    await flushPromises();
    resolveOld({ phase: 'old', status: 'failed', output: 'wrong task' });
    await flushPromises();
    expect(wrapper.text()).toContain('new task');
    expect(wrapper.text()).not.toContain('wrong task');
    wrapper.unmount();
    await vi.advanceTimersByTimeAsync(1000);
    expect(read).toHaveBeenCalledTimes(2);
  });
});
