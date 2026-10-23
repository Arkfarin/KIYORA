import asyncio,os,json,uuid,wave,math,struct,argparse,base64,hashlib,tarfile
from pathlib import Path

parser=argparse.ArgumentParser(description="Opt-in real Telegram acceptance; dedicated test vault only")
parser.add_argument("--state-dir",type=Path,required=True)
parser.add_argument("--binary",type=Path,required=True)
parser.add_argument("--tdjson",type=Path,required=True)
parser.add_argument("--create-test-vault",action="store_true")
args=parser.parse_args()
from pathlib import Path
root=args.state_dir.resolve()
async def main():
 env=os.environ.copy();env['KIYORA_TEST_ROOT']=str(root/'profiles');env['KIYORA_TDJSON']=str(args.tdjson.resolve())
 process=await asyncio.create_subprocess_exec(str(args.binary.resolve()),stdin=asyncio.subprocess.PIPE,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.DEVNULL,env=env)
 async def call(command,**args):
  wire={'version':1,'request':{'command':command}}
  if args: wire['request']['args']=args
  process.stdin.write(json.dumps(wire).encode()+b'\n');await process.stdin.drain()
  response=json.loads(await asyncio.wait_for(process.stdout.readline(),180))['result']
  if 'Err' in response: raise RuntimeError(command+':'+response['Err']['code'])
  return response['Ok'].get('result')
 async def vault(kind,**payload):
  req={'kind':kind}
  if payload: req['payload']=payload
  return await call('vault',session_id=session,request=req)
 try:
  profile=json.loads((root/'profile.json').read_text())['profile_id']
  status=await call('switch_profile',profile_id=profile);session=status['context']['session_id']
  for _ in range(60):
   if (await call('auth_status',session_id=session))['state']=='READY': break
   await asyncio.sleep(1)
  else: raise RuntimeError('AUTHORIZATION_TIMEOUT')
  print('PASS: TDLib persisted session reopened.',flush=True)
  manifest_path=root/'acceptance-vault.json'
  if status['context']['vault_id']:
   if not manifest_path.exists(): raise RuntimeError('TEST_VAULT_MANIFEST_REQUIRED')
   manifest=json.loads(manifest_path.read_text())
   if manifest != {'profile_id':profile,'vault_id':status['context']['vault_id']}: raise RuntimeError('TEST_VAULT_MISMATCH')
  if not status['context']['vault_id']:
   if not args.create_test_vault: raise RuntimeError('EXPLICIT_TEST_VAULT_CREATION_REQUIRED')
   print('Creating dedicated Kiyora acceptance vault.',flush=True)
   status=await call('create_vault',session_id=session,title='Kiyora isolated acceptance')
   manifest_path.write_text(json.dumps({'profile_id':profile,'vault_id':status['context']['vault_id']}));manifest_path.chmod(0o600)
  else: await vault('sync')
  print('PASS: vault initialization/catch-up.',flush=True)
  snap=await vault('verify');assert snap['result']['primary_healthy'] and snap['result']['backup_healthy']
  print('PASS: snapshot A/B server read-back.',flush=True)
  ids_path=root/'test-ids.json'
  if ids_path.exists(): ids=json.loads(ids_path.read_text())
  else:
   ids={k:str(uuid.uuid4()) for k in ['track','source','import','playlist','create_playlist','entry','add_entry','coordinator']}
   ids_path.write_text(json.dumps(ids));ids_path.chmod(0o600)
  settings=await call('query',session_id=session,query={'kind':'settings'})
  device=await call('query',session_id=session,query={'kind':'device'})
  if settings['result']['gc_device_id'] != device['result']['device_id']:
   if settings['result']['gc_device_id'] is not None: raise RuntimeError('TEST_COORDINATOR_CONFLICT')
   await vault('assign_gc_coordinator',op_id=ids['coordinator'],target={'kind':'this_device'},expected_previous=None,force_confirmed=False)
  path=root/'acceptance-tone.wav'
  with wave.open(str(path),'wb') as out:
   out.setnchannels(1);out.setsampwidth(2);out.setframerate(44100)
   out.writeframes(b''.join(struct.pack('<h',int(2000*math.sin(2*math.pi*440*i/44100))) for i in range(44100)))
  tracks=await call('library',session_id=session,query=None,offset=0,limit=500)
  if not any(track['track_id']==ids['track'] for track in tracks):
   result=await vault('import',path=str(path),track_id=ids['track'],source_id=ids['source'],op_id=ids['import'],allow_duplicate=False)
   assert result['kind']=='command' and result['result']['status']=='APPLIED', 'import not applied'
   print('PASS: real upload and reducer registration.',flush=True)
  else: print('PASS: previously uploaded test fixture retained.',flush=True)
  for op,entity,action,payload in [(ids['create_playlist'],ids['playlist'],'playlist.create',{'title':'Acceptance playlist'}),(ids['add_entry'],ids['playlist'],'playlist.track.add',{'track_id':ids['track'],'entry_id':ids['entry'],'after_entry_id':None})]:
   result=await vault('mutate',op_id=op,entity_id=entity,action={'action':action,'payload':payload})
   assert result['result']['status']=='APPLIED'
  print('PASS: playlist create/add and command results.',flush=True)
  if 'bulk_job' not in ids:
   for name in ['bulk_job','bulk_track_1','bulk_source_1','bulk_op_1','bulk_track_2','bulk_source_2','bulk_op_2']: ids[name]=str(uuid.uuid4())
   ids_path.write_text(json.dumps(ids));ids_path.chmod(0o600)
  files=[{'path':str(path),'track_id':ids[f'bulk_track_{i}'],'source_id':ids[f'bulk_source_{i}'],'op_id':ids[f'bulk_op_{i}'],'allow_duplicate':True} for i in [1,2]]
  await call('start_job',session_id=session,job_id=ids['bulk_job'],request={'kind':'bulk_import','payload':{'files':files}},resume=True)
  for _ in range(180):
   job=await call('job_status',session_id=session,job_id=ids['bulk_job'])
   if job['state']=='succeeded':break
   if job['state']!='running':raise RuntimeError('BULK_JOB_'+job['state'].upper())
   await asyncio.sleep(1)
  else:raise RuntimeError('BULK_JOB_TIMEOUT')
  assert job['completed']==2 and all(item['result']['Ok']['result']['status']=='APPLIED' for item in job['result']['result'])
  print('PASS: durable bulk import, per-file reducer results.',flush=True)
  if 'cover_track' not in ids:
   for name in ['cover_track','cover_source','cover_import']:ids[name]=str(uuid.uuid4())
   ids_path.write_text(json.dumps(ids));ids_path.chmod(0o600)
  png=base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jK1kAAAAASUVORK5CYII=')
  cover_path=root/'acceptance-cover.wav'
  apic=b'\0image/png\0\x03\0'+png
  frame=b'APIC'+struct.pack('>I',len(apic))+b'\0\0'+apic
  n=len(frame);tag=b'ID3\x03\0\0'+bytes([(n>>21)&127,(n>>14)&127,(n>>7)&127,n&127])+frame
  data=bytearray(path.read_bytes());data+=b'id3 '+struct.pack('<I',len(tag))+tag+(b'\0' if len(tag)%2 else b'')
  data[4:8]=struct.pack('<I',len(data)-8);cover_path.write_bytes(data)
  result=await vault('import',path=str(cover_path),track_id=ids['cover_track'],source_id=ids['cover_source'],op_id=ids['cover_import'],allow_duplicate=True)
  assert result['kind']=='command' and result['result']['status']=='APPLIED'
  cover=await call('artwork',session_id=session,track_id=ids['cover_track'])
  assert bytes(cover['bytes'])==png and cover['sha256']==hashlib.sha256(png).hexdigest()
  print('PASS: embedded cover upload and independent SHA-256 read-back.',flush=True)
  cancel_id=str(uuid.uuid4())
  await call('start_job',session_id=session,job_id=cancel_id,request={'kind':'vault','payload':{'kind':'sync'}},resume=False)
  await call('cancel_job',session_id=session,job_id=cancel_id)
  for _ in range(60):
   job=await call('job_status',session_id=session,job_id=cancel_id)
   if job['state']!='running':break
   await asyncio.sleep(0.1)
  assert job['state'] in ['cancelled','succeeded']
  print('PASS: job cancellation completed.' if job['state']=='cancelled' else 'PASS: sync finished before cancellation.',flush=True)
  await call('forget_job',session_id=session,job_id=cancel_id)
  await vault('sync')

  result=await vault('check_sources',track_id=ids['track']);assert result['result'][0]['availability']=='available'
  download=str(uuid.uuid4());await vault('download_for_offline',download_id=download,track_id=ids['track'],source_id=ids['source'],pinned=True)
  for _ in range(120):
   info=await vault('get_download_status',download_id=download,track_id=ids['track'],source_id=ids['source'])
   if info['result']['state']=='fully_cached': break
   if info['result']['state']!='downloading': raise RuntimeError('download:'+info['result']['state'])
   await asyncio.sleep(1)
  else: raise RuntimeError('DOWNLOAD_TIMEOUT')
  print('PASS: verified offline download.',flush=True)
  job_id=str(uuid.uuid4())
  await call('start_job',session_id=session,job_id=job_id,request={'kind':'export_backup','payload':{'path':str(root/f'live-full-{job_id}.tmv-backup'),'kind':'full'}},resume=False)
  for _ in range(180):
   job=await call('job_status',session_id=session,job_id=job_id)
   if job['state']=='succeeded': break
   if job['state']!='running': raise RuntimeError('EXPORT_JOB_'+job['state'].upper())
   await asyncio.sleep(1)
  else:raise RuntimeError('EXPORT_JOB_TIMEOUT')
  await call('forget_job',session_id=session,job_id=job_id)
  await call('export_backup',session_id=session,path=str(root/f'live-metadata-{job_id}.tmv-backup'),kind='metadata')
  print('PASS: metadata and full backup export.',flush=True)
  with tarfile.open(root/f'live-full-{job_id}.tmv-backup') as archive:
   assert archive.extractfile('artwork/'+ids['cover_track']).read()==png
  before=await call('storage_usage',session_id=session)
  after=await call('trim_telegram_storage',session_id=session,target_bytes=0)
  assert after['verified_cache']['pinned_entries']==before['verified_cache']['pinned_entries']
  await vault('verify')
  print('PASS: local TDLib storage cleanup; pinned cache and server snapshots preserved.',flush=True)
  await vault('snapshot');plan=await vault('plan_gc')
  print('PASS: committed snapshot and GC dry-run; no deletion yet.',flush=True)
  (root/'smoke-result.json').write_text(json.dumps({'passed':True,'gc_plan':plan}))
 finally:
  process.stdin.close()
  try: await asyncio.wait_for(process.wait(),45)
  except asyncio.TimeoutError:
   process.kill();await process.wait()
try: asyncio.run(main())
except Exception as e:
 print('STOP:',str(e) if isinstance(e,(RuntimeError,AssertionError)) else type(e).__name__,flush=True)
 raise SystemExit(1)
